//! `k5 app list`, `k5 app inspect` and `k5 app deployment`, as the shell
//! CLI's `cmd_app_list`, `cmd_app_inspect` and `cmd_app_deployment`
//! (`cli/src/app-doctor.sh`): one `_compute_call` GET each, `--json` as it
//! came, else the shell's Python line for line. `inspect --resources`
//! (the box usage view's printer) is still the shell's.

use super::call::Ctx;
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{ljust, Raise};
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// The usage lines.
pub mod usage {
    use super::cli_name;
    pub const DEPLOYMENT: &str = concat!(
        "usage: ",
        cli_name!(),
        " app deployment <app-id> [<deployment-id>]"
    );
    pub const INSPECT: &str = concat!(
        "usage: ",
        cli_name!(),
        " app inspect <app-id> [--resources] [--json]"
    );
}

/// `app inspect <app> [--json]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspect {
    pub app: String,
    pub json: bool,
}

/// `cmd_app_inspect`'s loop. `None`: a help form or `--resources` (the
/// shell's).
pub fn parse_inspect(words: &[&str]) -> Option<Result<Inspect, Stop>> {
    let mut i = Inspect {
        app: String::new(),
        json: false,
    };
    let mut resources = false;
    for w in words {
        match *w {
            "--json" => i.json = true,
            "--resources" => resources = true,
            "-h" | "--help" => return None,
            _ if w.starts_with('-') => return Some(Err(Stop::usage(format!("Unknown flag: {w}")))),
            _ => i.app = w.to_string(),
        }
    }
    if i.app.is_empty() {
        return Some(Err(Stop::usage(usage::INSPECT.to_string())));
    }
    if resources {
        return None;
    }
    Some(Ok(i))
}

pub fn inspect(ctx: &mut Ctx, i: &Inspect) -> Answer {
    let body = ctx.call(false, "/cli/app/inspect", &format!("app={}", quote(&i.app)))?;
    if i.json {
        return Ok(raw(&body));
    }
    let mut o = Out::default();
    let r = inspect_lines(&body, &mut o);
    Ok(o.done(r))
}

/// `cmd_app_inspect`'s Python.
pub fn inspect_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let a = py::idx(&v, "app", 4)?;
    let rt = py::idx(&v, "runtime", 4)?;
    let s = py::or(py::dget(&v, "liveSeed", 4)?, Value::Object(Vec::new()));
    let (id, state, label) = (
        py::idx(&a, "appId", 5)?,
        py::idx(&a, "state", 5)?,
        py::idx(&a, "label", 5)?,
    );
    o.line(&format!(
        "{}  {}  ({})",
        py::s(&id),
        py::s(&state),
        py::s(&label)
    ));
    if py::truthy(&py::dget(&a, "stateReason", 6)?) {
        o.line(&format!(
            "  why       {}",
            py::s(&py::idx(&a, "stateReason", 7)?)
        ));
    }
    o.line(&format!(
        "  workspace {}",
        py::s(&py::dget(&a, "workspaceHandle", 8)?)
    ));
    let url = py::dget(&a, "url", 9)?;
    let url = if py::truthy(&url) {
        py::s(&url)
    } else {
        format!(
            "local only: http://127.0.0.1:{}",
            py::s(&py::dget(&a, "frontPort", 9)?)
        )
    };
    o.line(&format!("  url       {url}"));
    if py::truthy(&py::dget(&a, "frontPort", 10)?) {
        o.line(&format!(
            "  front     127.0.0.1:{} (stable across deploys; your own web server proxies here)",
            py::s(&py::idx(&a, "frontPort", 11)?)
        ));
    }
    let memory = if py::dget(&a, "memoryLimitBytes", 12)? == Value::Null {
        "no per-App limit".to_string()
    } else {
        format!(
            "{} MiB",
            py::shr(&py::idx(&a, "memoryLimitBytes", 12)?, 20, 12)?
        )
    };
    o.line(&format!("  memory   {memory}"));
    let live = py::dget(&rt, "live", 13)?;
    let live = if py::truthy(&live) {
        format!(
            "{} on :{} (pid {})",
            py::s(&py::idx(&live, "deployment", 14)?),
            py::s(&py::idx(&live, "port", 14)?),
            py::s(&py::dget(&live, "pid", 14)?)
        )
    } else {
        "-".to_string()
    };
    o.line(&format!("  live      {live}"));
    let warm = py::dget(&rt, "warm", 15)?;
    if py::truthy(&warm) {
        let dep = py::idx(&warm, "deployment", 17)?;
        let port = py::idx(&warm, "port", 17)?;
        let until = py::dget(&rt, "watchUntilSecs", 17)?;
        o.line(&format!(
            "  warm      {} on :{} (watch window: {}s left)",
            py::s(&dep),
            py::s(&port),
            py::s(&until)
        ));
    }
    if py::truthy(&s) {
        let hash = py::head(&py::idx(&s, "hash", 19)?, 12, 19)?;
        let seed_id = py::idx(&s, "seedId", 19)?;
        let version = py::or(py::dget(&s, "version", 19)?, py::text(""));
        let signer = py::idx(&s, "signer", 19)?;
        o.line(&format!(
            "  seed      {}  {} {}  signer {}",
            py::s(&hash),
            py::s(&seed_id),
            py::s(&version),
            py::s(&signer)
        ));
        let cw = py::or(py::dget(&s, "createdWith", 20)?, Value::Object(Vec::new()));
        let (k, c, ch) = (
            py::dget(&cw, "k2", 21)?,
            py::dget(&cw, "catalog", 21)?,
            py::dget(&cw, "checker", 21)?,
        );
        o.line(&format!(
            concat!("  created   ", product_name!(), " {}, catalog {}, {}"),
            py::s(&k),
            py::s(&c),
            py::s(&ch)
        ));
        if py::truthy(&py::dget(&s, "sourceRef", 22)?) {
            o.line(&format!(
                "  commit    {}",
                py::s(&py::idx(&s, "sourceRef", 23)?)
            ));
        }
    }
    let d = py::dget(&v, "lastDeployment", 24)?;
    if py::truthy(&d) {
        let kind = py::idx(&d, "kind", 26)?;
        let id = py::idx(&d, "id", 26)?;
        let actor = py::idx(&d, "actor", 26)?;
        let result = py::idx(&d, "result", 26)?;
        let err = if py::truthy(&py::dget(&d, "error", 26)?) {
            py::concat(" \u{2014} ", &py::idx(&d, "error", 26)?, 26)?
        } else {
            String::new()
        };
        o.line(&format!(
            "  last      {} {} by {}: {}{err}",
            py::s(&kind),
            py::s(&id),
            py::s(&actor),
            py::s(&result)
        ));
    }
    let au = py::dget_or(&v, "audit", Value::Object(Vec::new()), 27)?;
    let audit = if py::truthy(&py::dget(&au, "verified", 28)?) {
        format!(
            "chain verifies ({} rows)",
            py::s(&py::dget(&au, "rows", 28)?)
        )
    } else {
        format!(
            "CHAIN BROKEN at row {}: {}",
            py::s(&py::dget(&au, "brokenAt", 28)?),
            py::s(&py::dget(&au, "reason", 28)?)
        )
    };
    o.line(&format!("  audit     {audit}"));
    Ok(Exit::Ok)
}

/// What `list` prints when there is nothing on the box.
pub const NO_APPS: &str = concat!(
    "No Apps on this box. Start with: ",
    cli_name!(),
    " study app-deploys"
);

/// `app list [--workspace <ws>] [--box] [--json]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    pub workspace: String,
    pub boxed: bool,
    pub json: bool,
}

/// `app deployment <app> [<id>] [--json]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployment {
    pub app: String,
    pub id: String,
    pub json: bool,
}

/// `cmd_app_list`'s loop. `None`: a help form (the shell's).
pub fn parse_list(words: &[&str]) -> Option<Result<List, Stop>> {
    let mut l = List {
        workspace: String::new(),
        boxed: false,
        json: false,
    };
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--workspace" => {
                // `shift 2` with one word left fails, and `set -e` ends
                // the command there with nothing said.
                if rest.len() < 2 {
                    return Some(Err(Stop::silent(Exit::Error)));
                }
                l.workspace = rest[1].to_string();
                rest = &rest[2..];
                continue;
            }
            "--box" => l.boxed = true,
            "--json" => l.json = true,
            "-h" | "--help" => return None,
            _ => return Some(Err(Stop::usage(format!("Unknown argument: {w}")))),
        }
        rest = &rest[1..];
    }
    Some(Ok(l))
}

/// `cmd_app_deployment`'s loop. `None`: a help form (the shell's).
pub fn parse_deployment(words: &[&str]) -> Option<Result<Deployment, Stop>> {
    let mut d = Deployment {
        app: String::new(),
        id: String::new(),
        json: false,
    };
    for w in words {
        match *w {
            "--json" => d.json = true,
            "-h" | "--help" => return None,
            _ if w.starts_with('-') => return Some(Err(Stop::usage(format!("Unknown flag: {w}")))),
            _ => {
                if d.app.is_empty() {
                    d.app = w.to_string();
                } else {
                    d.id = w.to_string();
                }
            }
        }
    }
    if d.app.is_empty() {
        return Some(Err(Stop::usage(usage::DEPLOYMENT.to_string())));
    }
    Some(Ok(d))
}

pub fn list(ctx: &mut Ctx, l: &List) -> Answer {
    let q = if l.boxed {
        "box=1".to_string()
    } else if !l.workspace.is_empty() {
        format!("workspace={}", quote(&l.workspace))
    } else {
        String::new()
    };
    let body = ctx.call(false, "/cli/app/list", &q)?;
    if l.json {
        return Ok(raw(&body));
    }
    let mut o = Out::default();
    let r = if l.boxed {
        box_lines(&body, &mut o)
    } else {
        list_lines(&body, &mut o)
    };
    Ok(o.done(r))
}

pub fn deployment(ctx: &mut Ctx, d: &Deployment) -> Answer {
    let mut q = format!("app={}", quote(&d.app));
    if !d.id.is_empty() {
        q.push_str(&format!("&id={}", quote(&d.id)));
    }
    let body = ctx.call(false, "/cli/app/deployment", &q)?;
    if d.json {
        return Ok(raw(&body));
    }
    let mut o = Out::default();
    let r = deployment_lines(&body, &mut o);
    Ok(o.done(r))
}

/// `"%-<n>s" % v`.
fn pad(v: &Value, n: usize) -> String {
    ljust(&py::s(v), n)
}

/// `cmd_app_list`'s Python (the apps view).
pub fn list_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let apps = py::dget_or(&v, "apps", py::empty_list(), 4)?;
    let legacy = py::or(py::dget(&v, "legacy", 5)?, py::empty_list());
    if !py::truthy(&apps) && !py::truthy(&legacy) {
        o.line(NO_APPS);
    }
    for a in py::iter(&apps, 8)? {
        let e = py::dget(&a, "engine", 11)?;
        let mut eng = String::new();
        if e != Value::Null {
            let mut mark = String::new();
            if py::truthy(&py::dget(&e, "rehostDue", 15)?) {
                let w = py::dget(&e, "rehost", 16)?;
                let known = ["running", "deferred", "failed"]
                    .iter()
                    .any(|k| w == py::text(k));
                mark = format!(
                    " [re-host {}]",
                    if known { py::s(&w) } else { "due".to_string() }
                );
            }
            let label = py::or(py::dget(&e, "label", 18)?, py::text("-"));
            eng = format!("  engine {}{mark}", py::s(&label));
        }
        let app_id = py::idx(&a, "appId", 19)?;
        let state = py::idx(&a, "state", 19)?;
        let ws = py::or(py::dget(&a, "workspaceHandle", 19)?, py::text(""));
        let seed = py::or(py::dget(&a, "liveSeed", 20)?, py::text("-"));
        let seed = py::head(&seed, 12, 20)?;
        let url = py::dget(&a, "url", 20)?;
        let url = if py::truthy(&url) {
            py::s(&url)
        } else {
            format!("127.0.0.1:{}", py::s(&py::dget(&a, "frontPort", 20)?))
        };
        o.line(&format!(
            "{} {} {} seed {}  {url}{eng}",
            pad(&app_id, 20),
            pad(&state, 12),
            pad(&ws, 14),
            py::s(&seed)
        ));
        let label = py::dget_or(&a, "label", py::text(""), 21)?;
        o.line(&format!("{} {}", ljust("", 20), py::s(&label)));
    }
    for l in py::iter(&legacy, 23)? {
        let moved = if py::truthy(&py::dget(&l, "labelMovedTo", 24)?) {
            format!(
                "  (its label now serves App {})",
                py::s(&py::idx(&l, "labelMovedTo", 24)?)
            )
        } else {
            String::new()
        };
        let name = py::dget(&l, "name", 25)?;
        let desired = py::dget_or(&l, "desired", py::text(""), 25)?;
        let ws = py::or(py::dget(&l, "workspaceHandle", 25)?, py::text(""));
        let label = py::dget_or(&l, "label", py::text(""), 25)?;
        o.line(&format!(
            "{} {} {} {}{moved}",
            pad(&name, 20),
            pad(&desired, 12),
            pad(&ws, 14),
            py::s(&label)
        ));
        let hint = py::dget_or(&l, "hint", py::text(""), 26)?;
        o.line(&format!("{} how: {}", ljust("", 20), py::s(&hint)));
    }
    let notices = py::dget_or(&v, "notices", py::empty_list(), 27)?;
    for n in py::iter(&notices, 27)? {
        let hint = py::dget_or(&n, "hint", py::text(""), 28)?;
        o.line(&py::concat("note: ", &hint, 28)?);
    }
    Ok(Exit::Ok)
}

/// `mib = lambda b: "-" if b is None else "%d MiB" % (b >> 20)`.
fn mib(b: &Value, line: u32) -> Result<String, Raise> {
    if *b == Value::Null {
        return Ok("-".to_string());
    }
    Ok(format!("{} MiB", py::shr(b, 20, line)?))
}

/// `pool and pool < 1 << 60`.
fn pool_known(pool: &Value, line: u32) -> Result<bool, Raise> {
    Ok(py::truthy(pool) && py::lt_2_60(pool, line)?)
}

/// `cmd_app_list --box`'s Python.
pub fn box_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let m = py::dget_or(&v, "memory", Value::Object(Vec::new()), 5)?;
    let pool = py::dget(
        &py::dget_or(&m, "pool", Value::Object(Vec::new()), 6)?,
        "poolBytes",
        6,
    )?;
    let used = mib(&py::dget(&m, "usedBytes", 7)?, 7)?;
    let total = if pool_known(&pool, 7)? {
        mib(&pool, 7)?
    } else {
        "unknown".to_string()
    };
    let free = if pool_known(&pool, 7)? {
        mib(&py::dget(&m, "freeBytes", 7)?, 7)?
    } else {
        "-".to_string()
    };
    o.line(&format!(
        "App memory on this box: {used} used of {total} ({free} free)"
    ));
    for a in py::iter(&py::dget_or(&v, "apps", py::empty_list(), 8)?, 8)? {
        let who = py::or(py::dget(&a, "maintainer", 9)?, Value::Object(Vec::new()));
        let by = if py::dget(&who, "kind", 10)? == py::text("agent") {
            format!(
                "{} ({})",
                py::s(&py::dget(&who, "agent", 10)?),
                py::s(&py::dget(&who, "address", 10)?)
            )
        } else {
            "parked: held by the owner".to_string()
        };
        let app_id = py::idx(&a, "appId", 11)?;
        let state = py::idx(&a, "state", 11)?;
        o.line(&format!("{} {} {by}", pad(&app_id, 20), pad(&state, 12)));
        let memory = py::idx(&a, "memory", 12)?;
        let mut extra = vec![format!(
            "memory {}",
            mib(&py::dget(&memory, "limitBytes", 12)?, 12)?
        )];
        let disk = py::dget_or(&a, "disk", Value::Object(Vec::new()), 13)?;
        if py::dget(&disk, "budgetBytes", 13)? != Value::Null {
            let budget = py::idx(&py::idx(&a, "disk", 14)?, "budgetBytes", 14)?;
            extra.push(format!("disk {}", mib(&budget, 14)?));
        }
        if py::truthy(&py::dget(&a, "hostnames", 15)?) {
            let hosts = py::join(", ", &py::idx(&a, "hostnames", 16)?, 16)?;
            extra.push(format!("hosts {hosts}"));
        }
        if py::truthy(&py::dget(&a, "ports", 17)?) {
            let mut ports = Vec::new();
            for p in py::iter(&py::idx(&a, "ports", 18)?, 18)? {
                ports.push(format!(
                    "{}/{}",
                    py::s(&py::idx(&p, "port", 18)?),
                    py::s(&py::idx(&p, "proto", 18)?)
                ));
            }
            extra.push(format!("ports {}", ports.join(", ")));
        }
        if py::truthy(&py::dget(&a, "boxServices", 19)?) {
            let services = py::join(", ", &py::idx(&a, "boxServices", 20)?, 20)?;
            extra.push(format!("box services {services}"));
        }
        o.line(&format!("{} {}", ljust("", 20), extra.join("  ")));
    }
    Ok(Exit::Ok)
}

/// `cmd_app_deployment`'s Python.
pub fn deployment_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let d = py::idx(&v, "deployment", 4)?;
    let app = py::idx(&v, "app", 5)?;
    let id = py::idx(&d, "id", 5)?;
    let kind = py::idx(&d, "kind", 5)?;
    o.line(&format!(
        "{} {} ({})",
        py::s(&app),
        py::s(&id),
        py::s(&kind)
    ));
    let state = py::idx(&v, "state", 6)?;
    let err = if py::truthy(&py::dget(&d, "error", 6)?) {
        py::concat("  \u{2014} ", &py::idx(&d, "error", 6)?, 6)?
    } else {
        String::new()
    };
    o.line(&format!("  state   {}{err}", py::s(&state)));
    o.line(&format!("  who     {}", py::s(&py::idx(&v, "who", 7)?)));
    let commit = py::or(py::dget(&v, "commit", 8)?, py::text("-"));
    o.line(&format!("  commit  {}", py::s(&commit)));
    let from = py::or(py::dget(&d, "fromSeed", 9)?, py::text("-"));
    let from = py::head(&from, 12, 9)?;
    let to = py::or(py::dget(&d, "toSeed", 9)?, py::text("-"));
    let to = py::head(&to, 12, 9)?;
    o.line(&format!(
        "  seed    {} \u{2192} {}",
        py::s(&from),
        py::s(&to)
    ));
    Ok(Exit::Ok)
}
