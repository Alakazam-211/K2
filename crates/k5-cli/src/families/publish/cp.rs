//! `publish`'s K2 Connect verbs (`cli/src/connect.sh`): `status` with no
//! name (`cmd_publish_status`) and `subdomain create`, `list`, `point`
//! (`target`), `rm` (`remove`, `delete`) and the usage for any other word
//! (`cmd_publish_subdomain`).
//!
//! Each control-plane call is `_connect_curl` ([`Run::connect_curl`]): the
//! air-gap check, then the daemon makes the call (`POST
//! /cli/connect/proxy`, [`crate::transport::Client::connect_proxy`]); only
//! when no daemon makes it (a refused connect, a daemon from before the
//! route) the control plane itself, with `tunnel.json`'s token as the
//! `Authorization` header and nowhere else (no child process sees it),
//! through the person's proxy for an `https://` base. A request that may
//! have reached the daemon is never sent again: the line, exit 1.
//!
//! `create`, `point` and `rm` ask this computer's daemon first whether
//! the workspace may take the label (`_subdomain_label_preflight`), and
//! after the control plane's answer record it (`_stamp_subdomain_attrib`)
//! and nudge the daemon's map (`_nudge_subdomain_refresh`); `create` and
//! `point` then probe a local target (`_warn_if_local_target_down`).
//!
//! Each reader prints what the shell's Python prints for `<status>\n<body>`
//! as `$(…)` holds it; where that Python raises, a Python 3.12 traceback
//! (no source lines) and exit 1. `status` (text) with no row for this
//! computer prints its two lines and exits 0.

use super::local::{py_int, strip};
use super::read::{compact, err_line, obj, s};
use super::{Act, Cx};
use crate::airgap::State;
use crate::exit::Exit;
use crate::families::wiki::publish as wp;
use crate::families::{get, py_get, py_iter, py_left, py_len, raised, without_nuls, Raise, Shown};
use crate::out;
use crate::out::json::Value;
use crate::out::py;
use crate::out::pylib::json::loads;
use crate::transport::encode::quote;
use crate::transport::remote::{self, Base};
use crate::transport::wire::Method;
use crate::transport::{Auth, Call, Channel, Client, Error};
use k2_client::brand::{CLI_NAME as CLI, PRODUCT_NAME as PRODUCT};
use k2_client::hosts::{DASHBOARD_URL, ZONE_SUFFIX};
use std::path::{Path, PathBuf};

/// `/cli/tunnel/subdomains/refresh`.
pub const REFRESH: &str = "/cli/tunnel/subdomains/refresh";
/// The claim and unclaim routes.
const CLAIM: &str = super::CLAIM;
const UNCLAIM: &str = super::UNCLAIM;

/// What the words ask the control plane for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    /// `status` with no name.
    Status { json: bool },
    /// `subdomain list`.
    List { json: bool },
    /// `subdomain create <label> --target <endpoint>`.
    Create {
        label: String,
        target: String,
        transfer: bool,
    },
    /// `subdomain point|target <label> <endpoint>`.
    Point {
        label: String,
        target: String,
        transfer: bool,
    },
    /// `subdomain rm|remove|delete <label>`.
    Rm { label: String },
}

/// `cmd_publish_status`'s words (with a name: [`Act::AppStatus`]).
pub fn parse_status(rest: &[&str]) -> Act {
    let line = format!("usage: {CLI} publish status [<name>] [--json]");
    let mut json = false;
    let mut app = String::new();
    for w in rest {
        match *w {
            "--json" => json = true,
            f if f.starts_with('-') => {
                return Act::Usage {
                    code: "usage",
                    hint: format!("unknown flag '{f}' \u{2014} {line}"),
                    json,
                }
            }
            a if app.is_empty() => app = a.to_string(),
            a => {
                return Act::Usage {
                    code: "usage",
                    hint: format!("unexpected argument '{a}' \u{2014} {line}"),
                    json,
                }
            }
        }
    }
    if app.is_empty() {
        Act::Connect(Form::Status { json })
    } else {
        Act::AppStatus { name: app, json }
    }
}

/// `cmd_publish_subdomain`'s words, `claim` and `unclaim` aside.
pub fn parse_subdomain(rest: &[&str]) -> Act {
    let action = match rest.first().copied() {
        None | Some("") => "list",
        Some(a) => a,
    };
    let words = rest.get(1..).unwrap_or_default();
    let lines = |t: String| Act::Lines(t, Exit::Error);
    match action {
        "create" | "point" | "target" => {
            let create = action == "create";
            let (mut label, mut target, mut transfer) = (String::new(), String::new(), false);
            let mut i = 0;
            while i < words.len() {
                let w = words[i];
                match w {
                    "--target" => {
                        // `_need_value "$@"`: the flag as the last word is a
                        // usage error, exit 2, nothing sent.
                        match words.get(i + 1) {
                            Some(t) => target = (*t).to_string(),
                            None => {
                                return Act::Lines(
                                    "Error: --target takes a value\n".to_string(),
                                    Exit::Usage,
                                )
                            }
                        }
                        i += 2;
                        continue;
                    }
                    "--transfer" => transfer = true,
                    f if f.starts_with("--") => {}
                    // A label never begins with '-' (so -h can't be one).
                    f if f.starts_with('-') => {
                        return Act::Lines(format!("Error: unknown flag '{f}'\n"), Exit::Usage)
                    }
                    a if label.is_empty() => label = a.to_string(),
                    a if !create && target.is_empty() => target = a.to_string(),
                    _ => {}
                }
                i += 1;
            }
            if label.is_empty() || target.is_empty() {
                return lines(if create {
                    format!("Usage: {CLI} publish subdomain create <label> --target <endpoint> [--transfer]\n  e.g. {CLI} publish subdomain create staging --target localhost:3000\n")
                } else {
                    format!("Usage: {CLI} publish subdomain point <label> --target <endpoint> [--transfer]\n       {CLI} publish subdomain target <label> <endpoint>\n")
                });
            }
            Act::Connect(if create {
                Form::Create {
                    label,
                    target,
                    transfer,
                }
            } else {
                Form::Point {
                    label,
                    target,
                    transfer,
                }
            })
        }
        "list" => {
            let mut json = false;
            for w in words {
                match *w {
                    "--json" => json = true,
                    f if f.starts_with('-') => {
                        return Act::Lines(format!("Error: unknown flag '{f}'\n"), Exit::Usage)
                    }
                    a => {
                        return Act::Lines(
                            format!("Error: unexpected argument '{a}'\n"),
                            Exit::Usage,
                        )
                    }
                }
            }
            Act::Connect(Form::List { json })
        }
        "rm" | "remove" | "delete" => {
            let label = words.first().copied().unwrap_or("");
            // A label never begins with '-' (so -h can't be one).
            if label.starts_with('-') {
                return Act::Lines(format!("Error: unknown flag '{label}'\n"), Exit::Usage);
            }
            if label.is_empty() {
                return lines(format!("Usage: {CLI} publish subdomain rm <label>\n"));
            }
            Act::Connect(Form::Rm {
                label: label.to_string(),
            })
        }
        _ => lines(format!(
            "Usage: {CLI} publish subdomain <create|list|point|target|rm|claim|unclaim> ...\n  create  <label> --target <endpoint>  Create a nested subdomain (attributed to this workspace)\n  list                                 List your subdomains\n  point   <label> --target <endpoint>  Repoint to a new endpoint\n  target  <label> <endpoint>           Same as point (change where the label points)\n  rm      <label>                      Remove a subdomain\n  claim   <label>                      Adopt an existing label for THIS workspace (daemon-local)\n  unclaim <label>                      Drop a label's workspace attribution (daemon-local)\n"
        )),
    }
}

/// `K2_CONNECT_BASE` (its K5_, K2_ and K2SO_ names, empty counts as
/// unset), else the hosted control plane.
pub fn connect_base() -> String {
    k2_client::env_names::CONNECT_BASE
        .iter()
        .filter_map(std::env::var_os)
        .find(|v| !v.is_empty())
        .map_or_else(
            || k2_client::hosts::CONNECT_BASE.to_string(),
            |v| v.to_string_lossy().into_owned(),
        )
}

/// `_k2_url_is_literal_loopback`: `[::1]` (with a port or not),
/// `localhost` in any case, or `127.x.y.z`, read from the URL's text.
pub fn literal_loopback(url: &str) -> bool {
    let h = url.split_once("://").map_or(url, |(_, r)| r);
    let h = h.split('/').next().unwrap_or("");
    let h = h.split('?').next().unwrap_or("");
    let h = h.split('#').next().unwrap_or("");
    let h = h.rsplit_once('@').map_or(h, |(_, h)| h);
    if h == "[::1]" || h.starts_with("[::1]:") {
        return true;
    }
    let h = h.split(':').next().unwrap_or("").to_ascii_lowercase();
    if h == "localhost" {
        return true;
    }
    let parts: Vec<&str> = h.split('.').collect();
    let octet = |p: &str| {
        !p.is_empty()
            && p.len() <= 3
            && p.bytes().all(|b| b.is_ascii_digit())
            && (p.len() == 1 || !p.starts_with('0'))
            && p.parse::<u16>().is_ok_and(|n| n <= 255)
    };
    parts.len() == 4 && parts[0] == "127" && parts[1..].iter().all(|&p| octet(p))
}

/// `_connect_base_check`: the line when the base is refused (exit 2).
pub fn base_refusal(url: &str) -> Option<String> {
    let scheme = match url.split_once("://") {
        Some((s, _)) => s.to_ascii_lowercase(),
        None => String::new(),
    };
    match scheme.as_str() {
        "https" => None,
        "http" if literal_loopback(url) => None,
        "http" => Some(remote::plain_http_line(url)),
        _ => Some(format!(
            "Error: K2_CONNECT_BASE must start with https:// (got: {url})"
        )),
    }
}

/// What the control-plane verbs may use, read before anything is sent.
pub struct Prep {
    pub base: String,
    pub airgap: (State, PathBuf),
    /// The control plane's base, for when no daemon makes the call (only
    /// with a token in `tunnel.json` and air-gap off).
    pub control: Option<Base>,
    pub home: PathBuf,
}

/// The checks before a control-plane verb: `None` hands the line to the
/// shell (a base or a proxy setting the binary doesn't send through
/// itself, when it may have to call the control plane itself).
pub fn prep(home: &Path) -> Option<Prep> {
    let base = connect_base();
    let (state, file) = crate::airgap::state(&crate::offline::Env::current())?;
    let mut control = None;
    if base_refusal(&base).is_none()
        && state == State::Off
        && wp::tunnel_token(&home.join("tunnel.json")).is_some()
    {
        if base.ends_with('/') {
            return None;
        }
        control = Some(remote::routed(&base)?);
    }
    Some(Prep {
        base,
        airgap: (state, file),
        control,
        home: home.to_path_buf(),
    })
}

/// A run of one control-plane verb.
pub(super) struct Run<'a> {
    pub(super) cx: &'a mut Cx,
    pub(super) prep: &'a Prep,
}

/// Prints `shown` and gives its exit as an `Err` when it is not 0.
fn show(shown: &Shown) -> Result<(), Exit> {
    match shown.print() {
        Exit::Ok => Ok(()),
        e => Err(e),
    }
}

fn stderr(text: &str) {
    let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
}

impl Run<'_> {
    /// The verb, to its exit.
    pub(super) fn run(&mut self, form: &Form) -> Exit {
        match self.steps(form) {
            Ok(()) => Exit::Ok,
            Err(e) => e,
        }
    }

    fn steps(&mut self, form: &Form) -> Result<(), Exit> {
        match form {
            Form::Status { json } | Form::List { json } => {
                self.base_check()?;
                let me = self_label(&self.prep.home.join("tunnel.json"));
                let raw = self.connect_curl(Method::Get, "/subdomains", "", *json)?;
                let list = matches!(form, Form::List { .. });
                let shown = match (list, *json) {
                    (false, true) => status_json(&raw, &me),
                    (false, false) => render(&raw, |d| status_text(d, &me)),
                    (true, true) => list_json(&raw, &me),
                    (true, false) => render(&raw, |d| list_text(d, &me)),
                };
                show(&shown)
            }
            Form::Create {
                label,
                target,
                transfer,
            }
            | Form::Point {
                label,
                target,
                transfer,
            } => {
                let create = matches!(form, Form::Create { .. });
                if let Some(line) = wp::reserved_refusal(label) {
                    stderr(&line);
                    return Err(Exit::Error);
                }
                self.base_check()?;
                let extra: &[&str] = if *transfer {
                    &["check=1", "transfer=1"]
                } else {
                    &["check=1"]
                };
                self.preflight(label, extra)?;
                let raw = if create {
                    let body = wp::create_body(label, target);
                    self.connect_curl(Method::Post, "/subdomains", &body, false)?
                } else {
                    let body = wp::point_body(target);
                    let path = format!("/subdomains/{}", quote(label));
                    self.connect_curl(Method::Put, &path, &body, false)?
                };
                let shown = if create {
                    render(&raw, created)
                } else {
                    render(&raw, pointed)
                };
                show(&shown)?;
                let mut pairs = vec![format!("label={}", quote(label))];
                if !target.is_empty() {
                    pairs.push(format!("target={}", quote(target)));
                }
                if *transfer {
                    pairs.push("transfer=1".to_string());
                }
                let resp = self.attrib(CLAIM, &pairs)?;
                if let Err(lines) = wp::stamp(resp.as_bytes(), label) {
                    stderr(&lines);
                    return Err(Exit::Error);
                }
                self.nudge()?;
                if let Some(w) = local_target_down(target) {
                    stderr(&w);
                }
                Ok(())
            }
            Form::Rm { label } => {
                self.base_check()?;
                if !label.eq_ignore_ascii_case("skin") {
                    self.preflight(label, &["check=1", "release=1"])?;
                }
                let path = format!("/subdomains/{}", quote(label));
                let raw = self.connect_curl(Method::Delete, &path, "", false)?;
                show(&render(&raw, removed))?;
                let resp = self.attrib(UNCLAIM, &[format!("label={}", quote(label))])?;
                if let Err(lines) = unstamp(&resp, label) {
                    stderr(&lines);
                    return Err(Exit::Error);
                }
                self.nudge()?;
                Ok(())
            }
        }
    }

    /// `_connect_base_check || exit 2`.
    fn base_check(&self) -> Result<(), Exit> {
        match base_refusal(&self.prep.base) {
            Some(line) => {
                out::stderr_line(&line);
                Err(Exit::Usage)
            }
            None => Ok(()),
        }
    }

    /// `_subdomain_attrib_post <verb> …`: `cli_send cli_post`, its answer
    /// as `$(…) || resp=""` holds it.
    fn attrib(&mut self, endpoint: &str, pairs: &[String]) -> Result<String, Exit> {
        let refs: Vec<&str> = pairs.iter().map(String::as_str).collect();
        let answer = self.cx.client.send(endpoint, Call::QueryPost(&refs));
        self.cx.held(answer, true)
    }

    /// `_subdomain_label_preflight <label> …`, exit 1 on a refusal.
    fn preflight(&mut self, label: &str, extra: &[&str]) -> Result<(), Exit> {
        let mut pairs = vec![format!("label={}", quote(label))];
        pairs.extend(extra.iter().map(|p| (*p).to_string()));
        let resp = self.attrib(CLAIM, &pairs)?;
        wp::preflight(resp.as_bytes()).map_err(|lines| {
            stderr(&lines);
            Exit::Error
        })
    }

    /// `_nudge_subdomain_refresh`: `cli_post_form`, everything hidden.
    fn nudge(&mut self) -> Result<(), Exit> {
        let sent = self
            .cx
            .client
            .call(Channel::Tcp, REFRESH, Call::PostForm(&[]));
        let passport = self.cx.passport;
        match sent {
            Ok(r) => {
                self.cx.closing.answer(&r, passport);
                Ok(())
            }
            Err(Error::NoAnswer(_)) => Err(Exit::Error),
            Err(_) => Ok(()),
        }
    }

    /// `cp_raw="$(_connect_curl <method> <endpoint> [body])" || exit $?`.
    fn connect_curl(
        &mut self,
        method: Method,
        endpoint: &str,
        body: &str,
        json: bool,
    ) -> Result<String, Exit> {
        let (state, file) = &self.prep.airgap;
        if let Some(mut line) = crate::airgap::refusal(*state, file) {
            line.push(b'\n');
            let _ = out::write_flushed(&mut std::io::stderr().lock(), &line);
            return Err(Exit::Error);
        }
        let req = wp::proxy_request(method.as_str(), endpoint, body);
        match self
            .cx
            .client
            .connect_proxy(method == Method::Get, req.as_bytes())
        {
            Ok(r) => match r.status {
                200 => {
                    return relayed(&r.body).map_err(|e| {
                        out::stderr_line(&format!(
                            "Error: unexpected reply from the {PRODUCT} daemon for a {PRODUCT} Connect request ({e})"
                        ));
                        Exit::Error
                    })
                }
                404 | 405 => {}
                code => {
                    let (ecode, emsg, exit) = refused(code, &r.body);
                    out::stderr_line(&err_line(&ecode, &emsg, json));
                    return Err(exit);
                }
            },
            Err(e @ Error::MaybeApplied { .. }) => {
                out::stderr_line(&e.to_string());
                return Err(Exit::Error);
            }
            Err(_) => {}
        }
        let home = &self.prep.home;
        let token = wp::tunnel_token(&home.join("tunnel.json"));
        let (Some(token), Some(base)) = (token, self.prep.control.as_ref()) else {
            let hint = format!(
                "could not reach the {PRODUCT} daemon, which holds the {PRODUCT} Connect tunnel token ({}/tunnel.json has none). Start {PRODUCT} and try again \u{2014} or, if this device is not paired, choose an address in Settings \u{2192} Network \u{2192} Tunnel (or run '{CLI} tunnel start') first.",
                home.display()
            );
            out::stderr_line(&err_line("no_token", &hint, json));
            return Err(Exit::Error);
        };
        let mut cp = Client::new(&base.host, base.port, "", Auth::Owner(token.clone()));
        remote::apply(&mut cp, base, token);
        match cp.control_plane_as(method, endpoint, body.as_bytes()) {
            Ok(r) => Ok(wp::control_plane_raw(r.status, &r.body)),
            Err(_) => {
                out::stderr_line(&format!(
                    "Error: could not reach the {PRODUCT} Connect control plane at {}.",
                    self.prep.base
                ));
                Err(Exit::Error)
            }
        }
    }
}

/// `str(e)` of the exception in an exception's last line
/// (`KeyError: 'status'` is `'status'`).
fn exc_text(last: &str) -> String {
    last.split_once(": ").map_or(last, |(_, m)| m).to_string()
}

/// `d[key]` as Python raises for it: the value, or `str(e)`.
fn index<'a>(d: &'a Value, key: &str) -> Result<&'a Value, String> {
    crate::out::pylib::ops::index(d, key, 0).map_err(|r| exc_text(&r.exception))
}

/// `_connect_curl_daemon`'s reading of a 200: the relayed answer as
/// `"%d\n%s" % (status, body)`, as `$(…)` holds it, or `str(e)` of what
/// its Python raised.
pub fn relayed(body: &[u8]) -> Result<String, String> {
    let raw = String::from_utf8_lossy(&without_nuls(body)).into_owned();
    let d = loads(&raw).map_err(|f| f.message)?;
    let status = py_int(index(&d, "status")?).map_err(|e| exc_text(&e))?;
    let b = index(&d, "body")?;
    Ok(held(format!("{status}\n{}", py::str_of(b))))
}

/// `$(…)`: the trailing newlines dropped.
fn held(mut t: String) -> String {
    while t.ends_with('\n') {
        t.pop();
    }
    t
}

/// A refusal from the daemon (not 200, 404 or 405): `_publish_err`'s code
/// and hint, and the exit (3 for a 403 with no code: needs the owner).
pub fn refused(status: u16, body: &[u8]) -> (String, String, Exit) {
    let raw = String::from_utf8_lossy(&without_nuls(body)).into_owned();
    let pairs = match loads(&raw) {
        Ok(Value::Object(p)) => p,
        _ => Vec::new(),
    };
    let text = |v: Option<&Value>| match v.filter(|v| py::truthy(v)) {
        Some(v) => py::str_of(v),
        None => String::new(),
    };
    let code = text(get(&pairs, "code"));
    let err = match get(&pairs, "error") {
        Some(Value::Object(e)) => {
            let h = get(e, "hint").filter(|v| py::truthy(v));
            text(h.or_else(|| get(e, "code")))
        }
        other => text(other),
    };
    // `pair="$(…)"`, then split at the first newline.
    let pair = held(format!("{code}\n{err}\n")).replace('\0', "");
    let (ecode, emsg) = match pair.split_once('\n') {
        Some((c, m)) => (c.to_string(), m.to_string()),
        None => (pair.clone(), pair.clone()),
    };
    if status == 403 && ecode.is_empty() {
        return (
            "forbidden".to_string(),
            format!("{PRODUCT} Connect commands need the server owner, an Owner/Admin login, or a {PRODUCT} agent session's own passport; this credential was refused."),
            Exit::Refused,
        );
    }
    let ecode = if ecode.is_empty() {
        format!("http_{status}")
    } else {
        ecode
    };
    let emsg = if emsg.is_empty() {
        format!("the {PRODUCT} daemon refused the {PRODUCT} Connect request (HTTP {status})")
    } else {
        emsg
    };
    (ecode, emsg, Exit::Error)
}

/// `_connect_self_subdomain`: `tunnel.json`'s `subdomain`, stripped; any
/// failure is none.
pub fn self_label(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let Ok(t) = String::from_utf8(bytes) else {
        return String::new();
    };
    match loads(&t) {
        Ok(Value::Object(p)) => match get(&p, "subdomain") {
            Some(v @ Value::Str(_)) => strip(&py::str_of(v)).to_string(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// The status line and body of `<status>\n<body>`.
enum Cp {
    /// No newline.
    NoResponse,
    /// The status line is no number.
    BadStatus(String),
    /// The status and the body read as JSON (`{}` when blank or not JSON),
    /// and the body's text.
    Answer(i128, Value, String),
}

fn split(raw: &str) -> Cp {
    let Some((status, body)) = raw.split_once('\n') else {
        return Cp::NoResponse;
    };
    let status = strip(status);
    let Some(code) = super::local::int_text(status) else {
        return Cp::BadStatus(status.to_string());
    };
    let d = if body.chars().all(crate::families::py_space) {
        Value::Object(Vec::new())
    } else {
        loads(body).unwrap_or_else(|_| Value::Object(Vec::new()))
    };
    Cp::Answer(code, d, body.to_string())
}

/// What an exec'd snippet printed, or what it printed before it raised.
type Pretty = Result<String, (String, Raise)>;

/// `_connect_subdomain_render <pretty>` on `raw`.
fn render(raw: &str, pretty: impl Fn(&Value) -> Pretty) -> Shown {
    let (code, d, body) = match split(raw) {
        Cp::NoResponse => {
            return Shown::err(
                "Error: no response from the control plane (network down?)\n",
                Exit::Error,
            )
        }
        Cp::BadStatus(st) => {
            return Shown::err(
                format!("Error: bad status line {}\n", py::repr(&s(&st))),
                Exit::Error,
            )
        }
        Cp::Answer(c, d, b) => (c, d, b),
    };
    if (200..300).contains(&code) {
        return match pretty(&d) {
            Ok(t) => Shown::out(t),
            Err((t, r)) => raised(t, String::new(), &r),
        };
    }
    let field = |k: &str, line: u32| -> Result<String, Raise> {
        let Value::Object(p) = &d else {
            return Ok(String::new());
        };
        match get(p, k).filter(|v| py::truthy(v)) {
            None => Ok(String::new()),
            Some(Value::Str(t)) => Ok(strip(&py::str_of(&Value::Str(t.clone()))).to_string()),
            Some(other) => Err(Raise::no_attribute(line, other, "strip")),
        }
    };
    let err = match field("error", 27) {
        Ok(e) => e,
        Err(r) => return raised(String::new(), String::new(), &r),
    };
    let detail = match field("detail", 28) {
        Ok(e) => e,
        Err(r) => return raised(String::new(), String::new(), &r),
    };
    if err == "pro_required" {
        let sub = if detail.is_empty() {
            "This subdomain".to_string()
        } else {
            detail
        };
        return Shown::err(
            format!("{sub} needs the Pro plan to create nested subdomains.\n  Upgrade your plan on the dashboard: {DASHBOARD_URL}\n"),
            Exit::Error,
        );
    }
    let known = match err.as_str() {
        "bad_label" => Some("Invalid label (use lowercase letters, digits, and dashes).".to_string()),
        "bad_target" => Some("Invalid target endpoint (e.g. localhost:3000).".to_string()),
        "label_taken" => Some("That label is already taken.".to_string()),
        "label_is_primary" => Some("That label is your primary subdomain \u{2014} it cannot be reassigned here.".to_string()),
        "primary_undeletable" => Some("Your primary subdomain cannot be removed.".to_string()),
        "not_found" => Some("No such subdomain.".to_string()),
        "invalid_token" => Some(format!("Tunnel token rejected \u{2014} choose your address again in Settings \u{2192} Network \u{2192} Tunnel ({} account \u{2192} Address) to refresh it.", k2_client::hosts::ZONE)),
        _ => None,
    };
    let m = match known {
        None if code == 401 => {
            "Unauthorized \u{2014} your tunnel token is missing or expired.".to_string()
        }
        None if !err.is_empty() => {
            let tail = if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            };
            format!("Control plane error ({err}){tail}")
        }
        None => {
            let tail = if body.chars().all(crate::families::py_space) {
                String::new()
            } else {
                format!(": {}", body.chars().take(200).collect::<String>())
            };
            format!("Control plane returned HTTP {code}{tail}")
        }
        Some(m) if !detail.is_empty() => format!("{m} ({detail})"),
        Some(m) => m,
    };
    Shown::err(format!("Error: {m}\n"), Exit::Error)
}

/// `s.get("host") or (str(s.get("label","?")) + ".k2.dev")`.
fn host_of(sv: &Value, line: u32) -> Result<Value, Raise> {
    if let Some(h) = py_get(sv, "host", line)?.filter(|h| py::truthy(h)) {
        return Ok(h.clone());
    }
    let label = py_get(sv, "label", line)?.map_or_else(|| "?".to_string(), py::str_of);
    Ok(s(&format!("{label}{ZONE_SUFFIX}")))
}

/// `x.strip().lower()` on a value Python must find a string.
fn strip_lower(v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(strip(&py::str_of(v)).to_lowercase()),
        other => Err(Raise::no_attribute(line, other, "strip")),
    }
}

/// `status`'s `label_of(s)`: the label, else the host's first part.
fn status_label_of(sv: &Value) -> Result<String, Raise> {
    if let Some(l) = py_get(sv, "label", 40)?.filter(|l| py::truthy(l)) {
        return Ok(strip(&py::str_of(l)).to_lowercase());
    }
    let h = host_of(sv, 41)?;
    match &h {
        Value::Str(_) => {
            let t = py::str_of(&h);
            Ok(strip(t.split('.').next().unwrap_or("")).to_lowercase())
        }
        other => Err(Raise::no_attribute(41, other, "split")),
    }
}

/// `list`'s `label_of(s)`: the label, else `(s.get("host") or "")`'s first
/// part.
fn list_label_of(sv: &Value) -> Result<String, Raise> {
    if let Some(l) = py_get(sv, "label", 40)?.filter(|l| py::truthy(l)) {
        return Ok(strip(&py::str_of(l)).to_lowercase());
    }
    match py_get(sv, "host", 41)?.filter(|h| py::truthy(h)) {
        None => Ok(String::new()),
        Some(h @ Value::Str(_)) => {
            let t = py::str_of(h);
            Ok(strip(t.split('.').next().unwrap_or("")).to_lowercase())
        }
        Some(other) => Err(Raise::no_attribute(41, other, "split")),
    }
}

/// `d.get("subdomains", []) if isinstance(d, dict) else []`, walked.
fn subs_of(d: &Value, line: u32) -> Result<Vec<Value>, Raise> {
    match d {
        Value::Object(p) => match get(p, "subdomains") {
            None => Ok(Vec::new()),
            Some(v) => py_iter(v, line),
        },
        _ => Ok(Vec::new()),
    }
}

/// `s.get(a) or s.get(b) or s.get(c)` on the connected keys.
fn marked(sv: &Value, line: u32) -> Result<bool, Raise> {
    for k in ["connected", "current", "is_current"] {
        if py_get(sv, k, line)?.is_some_and(py::truthy) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The row for this computer: the one the server marks, else the one
/// whose label is `me`.
fn status_row(subs: &[Value], me: &str) -> Result<Option<Value>, Raise> {
    for sv in subs {
        if marked(sv, 45)? {
            return Ok(Some(sv.clone()));
        }
    }
    if !me.is_empty() {
        for sv in subs {
            if status_label_of(sv)? == me {
                return Ok(Some(sv.clone()));
            }
        }
    }
    Ok(None)
}

/// `(row.get("tier") or "").strip().lower()`.
fn tier_of(sv: &Value, line: u32) -> Result<String, Raise> {
    match py_get(sv, "tier", line)?.filter(|t| py::truthy(t)) {
        None => Ok(String::new()),
        Some(t) => strip_lower(t, line),
    }
}

/// `{"pro": "Pro", ...}.get(tier, tier or None)`.
fn plan_value(tier: &str) -> Value {
    match tier {
        "pro" => s("Pro"),
        "single" => s("Single"),
        "free" => s("Free"),
        "" => Value::Null,
        t => s(t),
    }
}

fn or_null(t: &str) -> Value {
    if t.is_empty() {
        Value::Null
    } else {
        s(t)
    }
}

/// The JSON readers' check of `<status>\n<body>`: the body, or the
/// error's line (JSON on stderr) and exit 1.
fn json_answer(raw: &str) -> Result<Value, Shown> {
    let fail = |code: &str, hint: Value| {
        let line = compact_default(&obj(vec![(
            "error",
            obj(vec![("code", s(code)), ("hint", hint)]),
        )]));
        Shown::err(format!("{line}\n"), Exit::Error)
    };
    match split(raw) {
        Cp::NoResponse => Err(fail("unreachable", s("no response from control plane"))),
        Cp::BadStatus(st) => Err(fail("bad_status", s(&st))),
        Cp::Answer(code, d, body) if !(200..300).contains(&code) => {
            let p = match &d {
                Value::Object(p) => Some(p.as_slice()),
                _ => None,
            };
            let err = match p.and_then(|p| get(p, "error")).filter(|e| py::truthy(e)) {
                Some(e) => py::str_of(e),
                None => format!("http_{code}"),
            };
            let hint = match p.and_then(|p| get(p, "detail")).filter(|e| py::truthy(e)) {
                Some(h) => h.clone(),
                None => s(&body.chars().take(200).collect::<String>()),
            };
            Err(fail(&err, hint))
        }
        Cp::Answer(_, d, _) => Ok(d),
    }
}

/// `json.dumps(v)`: Python's default layout.
fn compact_default(v: &Value) -> String {
    super::read::dumps(v)
}

/// `status --json`.
fn status_json(raw: &str, me: &str) -> Shown {
    let d = match json_answer(raw) {
        Ok(d) => d,
        Err(shown) => return shown,
    };
    let me = strip(me).to_lowercase();
    match status_value(&d, &me) {
        Ok(v) => Shown::out(format!("{}\n", compact(&v))),
        Err(r) => raised(String::new(), String::new(), &r),
    }
}

/// `status --json`'s object.
fn status_value(d: &Value, me: &str) -> Result<Value, Raise> {
    let subs = subs_of(d, 30)?;
    let Some(row) = status_row(&subs, me)? else {
        let host = if me.is_empty() {
            Value::Null
        } else {
            s(&format!("{me}{ZONE_SUFFIX}"))
        };
        return Ok(obj(vec![
            ("connected", Value::Bool(!me.is_empty())),
            ("host", host),
            ("plan", Value::Null),
            ("tier", Value::Null),
            ("nested_subdomains", Value::Bool(false)),
        ]));
    };
    let tier = tier_of(&row, 60)?;
    Ok(obj(vec![
        ("connected", Value::Bool(true)),
        ("host", host_of(&row, 63)?),
        ("plan", plan_value(&tier)),
        ("tier", or_null(&tier)),
        ("nested_subdomains", Value::Bool(tier == "pro")),
    ]))
}

/// `status`'s snippet (text).
fn status_text(d: &Value, me: &str) -> Pretty {
    let me = strip(me).to_lowercase();
    let no = |r: Raise| (String::new(), r);
    let subs = subs_of(d, 4).map_err(no)?;
    let row = status_row(&subs, &me).map_err(no)?;
    let Some(row) = row else {
        let printed = if me.is_empty() {
            format!(
                "Not connected to a {PRODUCT} Connect subdomain.\n  Choose an address in Settings \u{2192} Network \u{2192} Tunnel (or run {CLI} tunnel start).\n"
            )
        } else {
            format!("Connected: {me}{ZONE_SUFFIX}\n  Plan: unknown (subdomain not found in your account list)\n")
        };
        // The snippet's `sys.exit(0)`: its two lines, exit 0.
        return Ok(printed);
    };
    let host = host_of(&row, 35).map_err(no)?;
    let tier = tier_of(&row, 36).map_err(no)?;
    let plan = match tier.as_str() {
        "pro" => "Pro \u{2713} (nested subdomains enabled)".to_string(),
        "single" => "Single \u{2014} upgrade to Pro to create nested subdomains".to_string(),
        "free" => "Free".to_string(),
        "" => "unknown".to_string(),
        t => capitalize(t),
    };
    let mut out = format!("Connected: {} \u{b7} Plan: {plan}\n", py::str_of(&host));
    if matches!(tier.as_str(), "single" | "free") {
        out.push_str(&format!(
            "  Manage your plan on the dashboard: {DASHBOARD_URL}\n"
        ));
    }
    Ok(out)
}

/// Python's `str.capitalize()`.
fn capitalize(t: &str) -> String {
    let mut c = t.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// One row of `list`, as the readers build it: host, flags, and the
/// values each reader adds.
fn flags_of(sv: &Value, me: &str) -> Result<Vec<&'static str>, Raise> {
    let mut flags = Vec::new();
    if py_get(sv, "primary", 50)?.is_some_and(py::truthy) {
        flags.push("primary");
    }
    if marked(sv, 52)? || (!me.is_empty() && list_label_of(sv)? == me) {
        flags.push("connected");
    }
    Ok(flags)
}

/// `subdomain list --json`.
fn list_json(raw: &str, me: &str) -> Shown {
    let d = match json_answer(raw) {
        Ok(d) => d,
        Err(shown) => return shown,
    };
    let me = strip(me).to_lowercase();
    match list_value(&d, &me) {
        Ok(v) => Shown::out(format!("{}\n", compact(&v))),
        Err(r) => raised(String::new(), String::new(), &r),
    }
}

/// `subdomain list --json`'s object.
fn list_value(d: &Value, me: &str) -> Result<Value, Raise> {
    let subs = subs_of(d, 45)?;
    let mut rows = Vec::new();
    for sv in &subs {
        let flags = flags_of(sv, me)?;
        let host = host_of(sv, 56)?;
        let target = match py_get(sv, "target", 57)?.filter(|v| py::truthy(v)) {
            Some(v) => v.clone(),
            None => s(""),
        };
        let tier = tier_of(sv, 58)?;
        let status = match py_get(sv, "effective", 60)?.filter(|v| py::truthy(v)) {
            Some(v) => v.clone(),
            None => match py_get(sv, "status", 60)?.filter(|v| py::truthy(v)) {
                Some(v) => v.clone(),
                None => s(""),
            },
        };
        let label = py_get(sv, "label", 62)?.cloned().unwrap_or(Value::Null);
        rows.push(obj(vec![
            ("host", host),
            ("target", target),
            ("plan", plan_value(&tier)),
            ("tier", or_null(&tier)),
            ("status", status),
            ("flags", Value::Array(flags.into_iter().map(s).collect())),
            ("label", label),
        ]));
    }
    Ok(obj(vec![("subdomains", Value::Array(rows))]))
}

/// `subdomain list`'s snippet (text).
fn list_text(d: &Value, me: &str) -> Pretty {
    let me = strip(me).to_lowercase();
    let no = |r: Raise| (String::new(), r);
    let subs_v = match d {
        Value::Object(p) => get(p, "subdomains")
            .cloned()
            .unwrap_or(Value::Array(Vec::new())),
        _ => Value::Array(Vec::new()),
    };
    if !py::truthy(&subs_v) {
        return Ok("No subdomains.\n".to_string());
    }
    let subs = py_iter(&subs_v, 18).map_err(no)?;
    let mut rows: Vec<[Value; 5]> = Vec::new();
    for sv in &subs {
        let flags = flags_of(sv, &me).map_err(no)?;
        let host = host_of(sv, 27).map_err(no)?;
        let target = py_get(sv, "target", 28)
            .map_err(no)?
            .map_or_else(String::new, py::str_of);
        let tier = tier_of(sv, 29).map_err(no)?;
        let plan = match tier.as_str() {
            "pro" => "Pro".to_string(),
            "single" => "Single".to_string(),
            "free" => "Free".to_string(),
            "" => "-".to_string(),
            t => t.to_string(),
        };
        let status = match py_get(sv, "effective", 30)
            .map_err(no)?
            .filter(|v| py::truthy(v))
        {
            Some(v) => py::str_of(v),
            None => py_get(sv, "status", 30)
                .map_err(no)?
                .map_or_else(String::new, py::str_of),
        };
        rows.push([host, s(&target), s(&plan), s(&status), s(&flags.join(" "))]);
    }
    let width = |i: usize, head: &str| -> Result<usize, Raise> {
        let mut w = head.chars().count();
        for r in &rows {
            w = w.max(py_len(&r[i], 33 + i as u32)?);
        }
        Ok(w)
    };
    let hw = width(0, "HOST").map_err(no)?;
    let tw = width(1, "TARGET").map_err(no)?;
    let pw = width(2, "PLAN").map_err(no)?;
    let sw = width(3, "STATUS").map_err(no)?;
    let mut out = format!(
        "  {}  {}  {}  {}  FLAGS\n",
        py_left(&s("HOST"), hw),
        py_left(&s("TARGET"), tw),
        py_left(&s("PLAN"), pw),
        py_left(&s("STATUS"), sw)
    );
    for r in &rows {
        out.push_str(&format!(
            "  {}  {}  {}  {}  {}\n",
            py_left(&r[0], hw),
            py_left(&r[1], tw),
            py_left(&r[2], pw),
            py_left(&r[3], sw),
            py::str_of(&r[4])
        ));
    }
    Ok(out)
}

/// `d.get(key, "?")` printed with `%s`, on a value Python must find a
/// dict.
fn or_q(d: &Value, key: &str, line: u32) -> Result<String, Raise> {
    Ok(py_get(d, key, line)?.map_or_else(|| "?".to_string(), py::str_of))
}

/// `create`'s snippet.
fn created(d: &Value) -> Pretty {
    let no = |r: Raise| (String::new(), r);
    let host = host_of(d, 2).map_err(no)?;
    let mut out = format!("Created {}\n", py::str_of(&host));
    for (k, name) in [
        ("target", "target:   "),
        ("status", "status:   "),
        ("cert_status", "cert:     "),
        ("effective", "effective:"),
    ] {
        match or_q(d, k, 4) {
            Ok(v) => out.push_str(&format!("  {name} {v}\n")),
            Err(r) => return Err((out, r)),
        }
    }
    Ok(out)
}

/// `point`'s snippet.
fn pointed(d: &Value) -> Pretty {
    let no = |r: Raise| (String::new(), r);
    let host = host_of(d, 2).map_err(no)?;
    let target = or_q(d, "target", 3).map_err(no)?;
    let mut out = format!("Repointed {} -> {target}\n", py::str_of(&host));
    for (k, name) in [("status", "status:   "), ("effective", "effective:")] {
        match or_q(d, k, 4) {
            Ok(v) => out.push_str(&format!("  {name} {v}\n")),
            Err(r) => return Err((out, r)),
        }
    }
    Ok(out)
}

/// `rm`'s snippet.
fn removed(d: &Value) -> Pretty {
    let r = py_get(d, "removed", 2).map_err(|r| (String::new(), r))?;
    let what = match r.filter(|v| py::truthy(v)) {
        Some(v) => py::str_of(v),
        None => "(subdomain)".to_string(),
    };
    Ok(format!("Removed {what}\n"))
}

/// `_stamp_subdomain_attrib unclaim <label>`'s readers.
fn unstamp(resp: &str, label: &str) -> Result<(), String> {
    let raw = if resp.is_empty() { "{}" } else { resp };
    let parsed = loads(raw);
    if matches!(&parsed, Ok(Value::Object(p)) if get(p, "success").is_some_and(py::truthy)) {
        return Ok(());
    }
    let err = match &parsed {
        Ok(Value::Object(p)) => get(p, "error").filter(|e| py::truthy(e)).map(py::str_of),
        _ => None,
    }
    .map_or_else(|| "no response from the daemon".to_string(), held);
    Err(format!(
        "Error: workspace attribution was not removed on the daemon: {err}\n  Remove it later: {CLI} publish subdomain unclaim {label}\n"
    ))
}

/// `_warn_if_local_target_down <target>`: the warning when the target is
/// this computer and nothing listens on its port (0.5 s per address).
pub fn local_target_down(target: &str) -> Option<String> {
    let mut t = strip(target);
    for p in ["http://", "https://"] {
        if let Some(r) = t.strip_prefix(p) {
            t = r;
        }
    }
    let t = t.split('/').next().unwrap_or("");
    let (host, port) = if let Some(r) = t.strip_prefix('[') {
        let (h, rest) = r.split_once(']').unwrap_or((r, ""));
        (h, rest.strip_prefix(':').unwrap_or(""))
    } else {
        match t.rsplit_once(':') {
            Some((h, p)) => (h, p),
            None => ("", t),
        }
    };
    let port = super::local::int_text(port)?;
    let h = host.to_lowercase();
    let h = h.trim_end_matches('.');
    let addrs: &[&str] = match h {
        "localhost" => &["127.0.0.1", "::1"],
        "127.0.0.1" => &["127.0.0.1"],
        "::1" => &["::1"],
        _ => return None,
    };
    let Ok(port) = u16::try_from(port) else {
        return Some(
            "Traceback (most recent call last):\n  File \"<stdin>\", line 30, in <module>\nOverflowError: connect(): port must be 0-65535.\n"
                .to_string(),
        );
    };
    let wait = std::time::Duration::from_millis(500);
    for ip in addrs {
        let ip: std::net::IpAddr = ip.parse().expect("a literal address");
        if std::net::TcpStream::connect_timeout(&(ip, port).into(), wait).is_ok() {
            return None;
        }
    }
    let tried: Vec<String> = addrs
        .iter()
        .map(|ip| {
            if ip.contains(':') {
                format!("[{ip}]:{port}")
            } else {
                format!("{ip}:{port}")
            }
        })
        .collect();
    Some(format!(
        "Warning: nothing is listening on {} yet. Visitors will see 502 (service not reachable) until your app is running on that port.\n",
        tried.join(" or ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_base_check_is_the_shell_s() {
        assert_eq!(base_refusal("https://cp.example"), None);
        assert_eq!(base_refusal("HTTPS://cp.example"), None);
        assert_eq!(base_refusal("http://127.0.0.1:9"), None);
        assert_eq!(base_refusal("http://LOCALHOST:9/x"), None);
        assert_eq!(base_refusal("http://[::1]:9"), None);
        assert!(base_refusal("http://127.0.0.256").is_some());
        assert!(base_refusal("http://cp.example").is_some());
        assert_eq!(
            base_refusal("cp.example").as_deref(),
            Some("Error: K2_CONNECT_BASE must start with https:// (got: cp.example)")
        );
    }

    #[test]
    fn status_with_no_row_prints_its_two_lines_and_exits_0() {
        let none = render("200\n{\"subdomains\":[]}", |d| status_text(d, " ACME "));
        assert_eq!(
            none,
            Shown::out(format!(
                "Connected: acme{ZONE_SUFFIX}\n  Plan: unknown (subdomain not found in your account list)\n"
            ))
        );
        let off = render("200\n{}", |d| status_text(d, ""));
        assert_eq!(
            off,
            Shown::out(format!(
                "Not connected to a {PRODUCT} Connect subdomain.\n  Choose an address in Settings \u{2192} Network \u{2192} Tunnel (or run {CLI} tunnel start).\n"
            ))
        );
    }

    #[test]
    fn the_relayed_answer() {
        assert_eq!(
            relayed(br#"{"status":201,"body":"{\"a\":1}\n"}"#),
            Ok("201\n{\"a\":1}".to_string())
        );
        assert_eq!(relayed(br#"{"body":""}"#), Err("'status'".to_string()));
        assert_eq!(
            relayed(b"[1]"),
            Err("list indices must be integers or slices, not str".to_string())
        );
        assert_eq!(
            relayed(br#"{"status":"x","body":""}"#),
            Err("invalid literal for int() with base 10: 'x'".to_string())
        );
        assert_eq!(
            relayed(b""),
            Err("Expecting value: line 1 column 1 (char 0)".to_string())
        );
    }

    #[test]
    fn a_refusal_s_pair() {
        let (c, m, e) = refused(500, br#"{"code":"x"}"#);
        assert_eq!((c.as_str(), m.as_str(), e), ("x", "x", Exit::Error));
        let (c, _, e) = refused(403, br#"{"error":"no"}"#);
        assert_eq!((c.as_str(), e), ("forbidden", Exit::Refused));
        let (c, m, _) = refused(502, b"nope");
        assert_eq!(c, "http_502");
        assert!(m.ends_with("(HTTP 502)"));
        let (c, m, _) = refused(400, br#"{"code":"c","error":{"code":"e"}}"#);
        assert_eq!((c.as_str(), m.as_str()), ("c", "e"));
    }

    #[test]
    fn the_words() {
        assert_eq!(
            parse_subdomain(&["target", "a", "localhost:1"]),
            Act::Connect(Form::Point {
                label: "a".into(),
                target: "localhost:1".into(),
                transfer: false
            })
        );
        assert_eq!(
            parse_subdomain(&["create", "a", "--target"]),
            Act::Lines("Error: --target takes a value\n".into(), Exit::Usage)
        );
        assert_eq!(
            parse_subdomain(&[]),
            Act::Connect(Form::List { json: false })
        );
        assert_eq!(
            parse_subdomain(&[""]),
            Act::Connect(Form::List { json: false })
        );
        assert!(matches!(
            parse_status(&["--json", "-x"]),
            Act::Usage { json: true, .. }
        ));
        assert_eq!(
            parse_status(&["", "web"]),
            Act::AppStatus {
                name: "web".into(),
                json: false
            }
        );
    }

    #[test]
    fn the_list_table() {
        let raw = "200\n{\"subdomains\":[{\"host\":\"a.k2.dev\",\"target\":\"localhost:1\",\"tier\":\"Pro\",\"primary\":true},{\"label\":\"me\",\"status\":\"ok\"}]}";
        let shown = render(raw, |d| list_text(d, "ME"));
        assert_eq!(
            shown.stdout,
            "  HOST       TARGET       PLAN  STATUS  FLAGS\n  a.k2.dev   localhost:1  Pro           primary\n  me.k2.dev               -     ok      connected\n"
        );
    }
}
