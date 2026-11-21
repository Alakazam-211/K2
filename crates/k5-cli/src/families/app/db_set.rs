//! The writes of `app db` (`cmd_app_db`, `cli/src/app-db.sh`): `connect`,
//! `check` and `disconnect`, with the word loop every `app db` verb
//! shares ([`words`]; `status` is [`super::walls`]' read).
//!
//! The exits are the family's own (R7): 0 done (PASS, or the check
//! skipped), 1 failed (FAIL, a usage mistake as one JSON line, or a 400
//! as `_app_db_fail`'s line), 2 held (one card for the owner), 3 refused.
//!
//! - `connect` POSTs once (curl's 900 seconds). A 202 is held: its card,
//!   exit 2; with `--wait` the card goes to stderr and the status is read
//!   every 5 seconds until the owner answers or `--timeout` runs out.
//! - `check` and `disconnect` POST once (curl's 300 seconds).
//! - `connect --read-only` (0.46.17) is the ask `db request-read` makes,
//!   with `connect` ([`crate::families::db::read::posted`]): a card that
//!   is up is held (2); once the caller reads the database, its read-only
//!   DSN with no password.
//!
//! Only ids and names are printed, never a URL.

use super::call::{Cj, Ctx};
use super::py::{self, Out};
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{py_len, Raise, Shown};
use crate::out;
use crate::out::json::{self, Options, Text, Value};
use crate::out::py as pyfmt;
use crate::transport::encode::quote;
use k2_client::cli_name;
use std::time::Duration;

/// `connect --read-only` with a flag that makes a new connection.
const READ_ONLY_ALONE: &str = concat!(
    "{\"error\":\"usage\",\"hint\":\"--read-only takes the App and, when it uses several databases, --db <name>; not --new, --major, --cluster, --name, --as, --no-migrate-url or --wait (",
    cli_name!(),
    " app db --help)\"}"
);
/// `--why` or `--assign` on a connect without `--read-only`.
const WHY_READ_ONLY: &str = concat!(
    "{\"error\":\"usage\",\"hint\":\"--why and --assign go with --read-only (an App connect takes --reason) (",
    cli_name!(),
    " app db --help)\"}"
);
/// `connect` with neither `--new` nor `--db`.
const WHICH_DB: &str = concat!(
    "{\"error\":\"usage\",\"hint\":\"say which database: --new or --db <name> (",
    cli_name!(),
    " app db --help)\"}"
);
/// `… (k2 app db status <app>)"}`.
const STATUS_HEAD: &str = concat!(" (", cli_name!(), " app db status ");

/// The words of an `app db <sub>` line, read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Words {
    pub app: String,
    pub json: bool,
    pub new: bool,
    pub db: String,
    pub major: String,
    pub cluster: String,
    pub name: String,
    pub r#as: String,
    pub no_migrate_url: bool,
    pub wait: bool,
    /// Seconds (`_compute_secs`), 600 when not given.
    pub timeout: i64,
    pub drop_roles: bool,
    pub reason: String,
    /// `--read-only` (0.46.17): another workspace's App database, read
    /// only, on a card ([`read_only`]).
    pub read_only: bool,
    pub why: String,
    pub assign: String,
}

/// `cmd_app_db`'s loop and checks for `sub`. A mistake is
/// `{"error":"usage","hint":…}` and exit 1; a bad `--timeout` is
/// `_compute_secs`' line and exit 2.
pub fn words(sub: &str, words: &[&str]) -> Result<Words, Stop> {
    let mut w = Words {
        timeout: 600,
        ..Words::default()
    };
    let mut bad = String::new();
    let mut rest = words;
    while let Some(word) = rest.first().copied() {
        match word {
            "--json" => w.json = true,
            "--new" => w.new = true,
            "--no-migrate-url" => w.no_migrate_url = true,
            "--wait" => w.wait = true,
            "--drop-roles" => w.drop_roles = true,
            "--read-only" => w.read_only = true,
            "--db" | "--major" | "--cluster" | "--name" | "--as" | "--timeout" | "--reason"
            | "--why" | "--assign" => {
                let v = rest.get(1).copied().unwrap_or("");
                if v.is_empty() {
                    bad = format!("{word} needs a value");
                    break;
                }
                let v = v.to_string();
                match word {
                    "--db" => w.db = v,
                    "--major" => w.major = v,
                    "--cluster" => w.cluster = v,
                    "--name" => w.name = v,
                    "--as" => w.r#as = v,
                    "--reason" => w.reason = v,
                    "--why" => w.why = v,
                    "--assign" => w.assign = v,
                    _ => w.timeout = compute_secs(&v)?,
                }
                rest = &rest[2..];
                continue;
            }
            _ if word.starts_with("--db=") => w.db = word["--db=".len()..].to_string(),
            _ if word.starts_with("--major=") => w.major = word["--major=".len()..].to_string(),
            _ if word.starts_with("--cluster=") => {
                w.cluster = word["--cluster=".len()..].to_string()
            }
            _ if word.starts_with("--as=") => w.r#as = word["--as=".len()..].to_string(),
            _ if word.starts_with('-') => {
                bad = format!("unknown flag {word}");
                break;
            }
            _ if w.app.is_empty() => w.app = word.to_string(),
            _ => {
                bad = format!("one App at a time (got {word} too)");
                break;
            }
        }
        rest = &rest[1..];
    }
    if bad.is_empty() && w.app.is_empty() {
        bad = format!("{}{sub} <app-id>", super::walls::usage::DB_NAME_APP);
    }
    if bad.is_empty() && !w.major.is_empty() && !two_digit_major(&w.major) {
        bad = "--major takes a Postgres major, like 17".to_string();
    }
    if !bad.is_empty() {
        let hint = json::to_string(
            &Value::Str(Text::from(
                format!("{bad}{}", super::walls::usage::DB_TAIL).as_str(),
            )),
            &Options::default(),
        )
        .unwrap_or_default();
        return Err(Stop {
            line: format!("{{\"error\":\"usage\",\"hint\":{hint}}}"),
            exit: Exit::Error,
        });
    }
    Ok(w)
}

/// `case "$major" in [1-9][0-9])`.
fn two_digit_major(m: &str) -> bool {
    let b = m.as_bytes();
    b.len() == 2 && (b'1'..=b'9').contains(&b[0]) && b[1].is_ascii_digit()
}

/// `_compute_secs <v>`: plain seconds, or `<n>h|m|s` parts in any mix
/// (bash's 64-bit arithmetic). Anything else is its usage line, and its
/// exit 2 fails the assignment, so set -e ends the command there.
pub fn compute_secs(v: &str) -> Result<i64, Stop> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    // `$((10#$v))`: base 10, wrapping as bash's arithmetic does.
    let num = |s: &str| {
        s.bytes().fold(0i64, |n, b| {
            n.wrapping_mul(10).wrapping_add(i64::from(b - b'0'))
        })
    };
    if digits(v) {
        return Ok(num(v));
    }
    let mut total = 0i64;
    let mut rest = v;
    let mut parts = 0;
    while !rest.is_empty() {
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        let mult = match rest.as_bytes().get(n) {
            Some(b'h') if n > 0 => 3600,
            Some(b'm') if n > 0 => 60,
            Some(b's') if n > 0 => 1,
            _ => {
                parts = 0;
                break;
            }
        };
        total = total.wrapping_add(num(&rest[..n]).wrapping_mul(mult));
        rest = &rest[n + 1..];
        parts += 1;
    }
    if parts > 0 {
        return Ok(total);
    }
    Err(Stop::usage(format!(
        "usage: bad duration '{v}' (use 2h, 90m, 1h30m, 300s)"
    )))
}

/// Which write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Write {
    Connect,
    Check,
    Disconnect,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub write: Write,
    pub words: Words,
}

/// The words after `db`. `None`: not a write here (or a help form).
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let write = match *words.first()? {
        // `--read-only`'s POST reads `K2_CP_MAX_TIME` as curl does: a
        // value the binary can't read the same way is the shell's.
        "connect"
            if words.contains(&"--read-only")
                && crate::families::compute::writes::post_max_time(std::env::var_os(
                    "K2_CP_MAX_TIME",
                ))
                .is_none() =>
        {
            return None
        }
        "connect" => Write::Connect,
        "check" => Write::Check,
        "disconnect" => Write::Disconnect,
        _ => return None,
    };
    Some(super::db_set::words(words[0], &words[1..]).map(|words| Ask { write, words }))
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    let w = &ask.words;
    match ask.write {
        Write::Check => {
            let body = Cj::new().s("app", &w.app).s("as", &w.r#as).body();
            let s = ctx.post_max("/cli/app/db/check", &body, 300)?;
            if !s.ok() {
                return Err(super::walls::db_fail(ctx, &s));
            }
            Ok(printed(&s.body, w.json))
        }
        Write::Disconnect => {
            let body = Cj::new()
                .s("app", &w.app)
                .s("as", &w.r#as)
                .b("dropRoles", if w.drop_roles { "1" } else { "0" })
                .body();
            let s = ctx.post_max("/cli/app/db/disconnect", &body, 300)?;
            if !s.ok() {
                return Err(super::walls::db_fail(ctx, &s));
            }
            if w.json {
                return Ok(json_exit(&s.body, |v| {
                    Ok(py::truthy(&py::dget(v, "ok", 1)?))
                }));
            }
            Ok(super::access::answer(&s.body, false, disconnect_lines))
        }
        Write::Connect => connect(ctx, w),
    }
}

/// `--json`: the answer as it came, then the one-liner's exit (0 when
/// `ok` says so, else 1; a raise is its traceback and 1).
fn json_exit(body: &[u8], ok: fn(&Value) -> Result<bool, Raise>) -> Shown {
    let mut shown = super::wallet::raw(body);
    let r = py::load(body, 1)
        .and_then(|v| ok(&v))
        .map(|ok| if ok { Exit::Ok } else { Exit::Error });
    let tail = Out::default().done(r);
    shown.stderr = tail.stderr;
    shown.exit = tail.exit;
    shown
}

/// `(json.load(sys.stdin).get("check") or {}).get("ok") is False` is a
/// failure.
fn check_not_false(v: &Value) -> Result<bool, Raise> {
    let c = py::or(py::dget(v, "check", 1)?, Value::Object(Vec::new()));
    Ok(py::dget(&c, "ok", 1)? != Value::Bool(false))
}

/// A connect or check answer: `--json` as it came and its check's exit,
/// else `_app_db_print`.
fn printed(body: &[u8], json: bool) -> Shown {
    if json {
        return json_exit(body, check_not_false);
    }
    super::access::answer(body, false, print_lines)
}

/// `connect --read-only`: the ask `db request-read` makes, with
/// `connect` (`_db_read_post … 2`): a card that is up is held, exit 2;
/// once the caller reads the database, its read-only DSN (no password).
fn read_only(ctx: &mut Ctx, w: &Words) -> Answer {
    if w.new
        || !format!("{}{}{}{}", w.major, w.cluster, w.name, w.r#as).is_empty()
        || w.no_migrate_url
        || w.wait
    {
        out::stderr_line(READ_ONLY_ALONE);
        return Err(Exit::Error);
    }
    let why = if w.why.is_empty() { &w.reason } else { &w.why };
    let body = Cj::new()
        .s("app", &w.app)
        .s("db", &w.db)
        .s("why", why)
        .s("assign", &w.assign)
        .b("connect", "1")
        .body();
    let max = crate::families::compute::writes::post_max_time(std::env::var_os("K2_CP_MAX_TIME"))
        .flatten();
    let before = std::mem::replace(&mut ctx.client.compute_post_max_time, max);
    let sent = ctx.send(true, "/cli/db/request-read", &body, false);
    ctx.client.compute_post_max_time = before;
    let s = sent?;
    Err(crate::families::db::read::posted(
        &s.status,
        &s.body,
        w.json,
        Exit::Usage,
    ))
}

fn connect(ctx: &mut Ctx, w: &Words) -> Answer {
    if w.read_only {
        return read_only(ctx, w);
    }
    if !w.why.is_empty() || !w.assign.is_empty() {
        out::stderr_line(WHY_READ_ONLY);
        return Err(Exit::Error);
    }
    if !w.new && w.db.is_empty() {
        out::stderr_line(WHICH_DB);
        return Err(Exit::Error);
    }
    let flag = |b: bool| if b { "1" } else { "0" };
    let body = Cj::new()
        .s("app", &w.app)
        .b("new", flag(w.new))
        .s("db", &w.db)
        .n("major", &w.major)
        .s("cluster", &w.cluster)
        .s("name", &w.name)
        .s("as", &w.r#as)
        .b("noMigrateUrl", flag(w.no_migrate_url))
        .s("reason", &w.reason)
        .body();
    let s = ctx.post_max("/cli/app/db/connect", &body, 900)?;
    match s.status.as_str() {
        "200" => return Ok(printed(&s.body, w.json)),
        "202" => {}
        _ => return Err(super::walls::db_fail(ctx, &s)),
    }
    // `id="$(… print(json.load(sys.stdin)["request"]["id"]))"`.
    let id = (|| -> Result<String, Raise> {
        let v = py::load(&s.body, 1)?;
        Ok(py::s(&py::idx(&py::idx(&v, "request", 1)?, "id", 1)?))
    })();
    let id = match id {
        Ok(id) => id.trim_end_matches('\n').to_string(),
        Err(e) => return Ok(Out::default().done(Err(e))),
    };
    if !w.wait {
        if w.json {
            let mut shown = super::wallet::raw(&s.body);
            shown.exit = Exit::Usage;
            return Ok(shown);
        }
        let mut shown = super::access::answer(&s.body, false, held_lines);
        if shown.exit == Exit::Ok {
            shown.exit = Exit::Usage;
        }
        return Ok(shown);
    }
    if !w.json {
        // The card on stderr; a raise there ends the command (set -e).
        // Python's stdout into a file is held until it exits, so a
        // traceback comes before the card's lines.
        let mut o = Out::default();
        let r = held_lines(&s.body, &mut o);
        let shown = o.done(r);
        let card = Shown::err(format!("{}{}", shown.stderr, shown.stdout), shown.exit);
        let exit = card.print();
        if exit != Exit::Ok {
            return Err(exit);
        }
    }
    out::stderr_line(&format!(
        "waiting for the owner (card {id}, up to {} min)\u{2026}",
        w.timeout / 60
    ));
    let query = format!("app={}&request={}", quote(&w.app), quote(&id));
    let mut waited: i64 = 0;
    while waited < w.timeout {
        std::thread::sleep(Duration::from_secs(5));
        waited += 5;
        let s = ctx.send(false, "/cli/app/db/status", &query, false)?;
        if !s.ok() {
            continue;
        }
        let st = (|| -> Result<String, Raise> {
            let v = py::load(&s.body, 1)?;
            let r = py::or(py::dget(&v, "request", 1)?, Value::Object(Vec::new()));
            Ok(py::s(&py::or(py::dget(&r, "status", 1)?, py::text(""))))
        })();
        let st = match st {
            Ok(st) => st.trim_end_matches('\n').to_string(),
            Err(e) => return Ok(Out::default().done(Err(e))),
        };
        match st.as_str() {
            "pending" | "" => continue,
            "approved" => {
                // `res="$(… print(json.dumps(…["request"]["result"])))"`.
                let res = (|| -> Result<String, Raise> {
                    let v = py::load(&s.body, 1)?;
                    let r = py::idx(&py::idx(&v, "request", 1)?, "result", 1)?;
                    Ok(json::to_string(&r, &Options::default()).unwrap_or_default())
                })();
                let res = match res {
                    Ok(res) => res,
                    Err(e) => return Ok(Out::default().done(Err(e))),
                };
                return Ok(printed(res.as_bytes(), w.json));
            }
            "failed" => {
                let r = (|| -> Result<String, Raise> {
                    let v = py::load(&s.body, 1)?;
                    let r = py::idx(&v, "request", 1)?;
                    let e = py::or(py::dget(&r, "result", 1)?, Value::Object(Vec::new()));
                    let code = py::dget_or(&e, "code", py::text("failed"), 1)?;
                    let hint = py::dget_or(&e, "hint", py::text(""), 1)?;
                    let line = Value::Object(vec![
                        (Text::from("error"), code),
                        (
                            Text::from("hint"),
                            Value::Str(Text::from(
                                format!("approved, but connecting failed: {}", py::s(&hint))
                                    .as_str(),
                            )),
                        ),
                    ]);
                    Ok(json::to_string(&line, &Options::default()).unwrap_or_default())
                })();
                return Ok(match r {
                    Ok(line) => Shown::err(format!("{line}\n"), Exit::Error),
                    Err(e) => Out::default().done(Err(e)),
                });
            }
            other => {
                out::stderr_line(&format!(
                    "{{\"error\":\"card_{other}\",\"hint\":\"the owner's card {id} ended {other}: nothing changed\"}}"
                ));
                return Err(Exit::Refused);
            }
        }
    }
    out::stderr_line(&format!(
        "{{\"error\":\"wait_timeout\",\"hint\":\"card {id} is still waiting for the owner{STATUS_HEAD}{})\"}}",
        w.app
    ));
    Err(Exit::Usage)
}

/// `env(n)` in `_app_db_print`: the secret's name without `app-db.<app>.`.
fn env_name(n: &Value, pre: &str, frames: &[(u32, &'static str)]) -> Result<String, Raise> {
    match n {
        Value::Str(_) => {
            let s = py::s(n);
            Ok(match s.strip_prefix(pre) {
                Some(rest) => rest.to_string(),
                None => s,
            })
        }
        other => Err(Raise {
            frames: frames.to_vec(),
            exception: format!(
                "AttributeError: '{}' object has no attribute 'startswith'",
                pyfmt::type_name(other)
            ),
        }),
    }
}

/// `_app_db_print`'s Python: exit 0 on PASS or a skipped check, 1 on FAIL.
pub fn print_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let app = py::dget_or(&v, "app", py::text(""), 4)?;
    let pre = format!("app-db.{}.", py::s(&app));
    let c = py::or(py::dget(&v, "check", 8)?, Value::Object(Vec::new()));
    let db = |line| py::dget(&v, "db", line);
    if py::contains(&v, "created", 9)? || py::contains(&v, "rehost", 9)? {
        let (d, major, cluster) = (
            db(10)?,
            py::dget(&v, "major", 10)?,
            py::dget(&v, "cluster", 10)?,
        );
        let new = if py::truthy(&py::dget(&v, "created", 10)?) {
            "  [new]"
        } else {
            ""
        };
        o.line(&format!(
            "connected {} \u{2192} {} (Postgres {}, cluster {}){new}",
            py::s(&app),
            py::s(&d),
            py::s(&major),
            py::s(&cluster)
        ));
    } else {
        let (d, major, cluster) = (
            db(12)?,
            py::dget(&v, "major", 12)?,
            py::dget(&v, "cluster", 12)?,
        );
        o.line(&format!(
            "{} \u{2192} {} (Postgres {}, cluster {})",
            py::s(&app),
            py::s(&d),
            py::s(&major),
            py::s(&cluster)
        ));
    }
    let roles = py::or(py::dget(&v, "roles", 13)?, py::empty_list());
    o.line(&format!("roles    {}", py::join(", ", &roles, 13)?));
    let secrets = py::or(py::dget(&v, "secrets", 14)?, py::empty_list());
    let mut names = Vec::new();
    for s in py::iter(&secrets, 14)? {
        names.push(env_name(
            &s,
            &pre,
            &[(14, "<module>"), (14, "<genexpr>"), (7, "env")],
        )?);
    }
    o.line(&format!(
        "secrets  {} (granted to the App; not shown)",
        names.join(", ")
    ));
    let kind = py::dget(&v, "kind", 15)?;
    let door = if py::truthy(&py::dget(&v, "doorOpen", 16)?) {
        "open"
    } else {
        "SHUT"
    };
    let where_ = match &kind {
        Value::Array(_) | Value::Object(_) => {
            return Err(Raise::at(
                17,
                format!("TypeError: unhashable type: '{}'", pyfmt::type_name(&kind)),
            ))
        }
        k if *k == py::text("container") => "container door, in-wall 127.0.0.1:5432".to_string(),
        k if *k == py::text("process-exception") => "its nft rule, the cluster port".to_string(),
        k if *k == py::text("process") => "the daemon network".to_string(),
        k => py::s(k),
    };
    o.line(&format!("door     {door} ({where_})"));
    let replaced = py::or(py::dget(&v, "replaced", 19)?, py::empty_list());
    for r in py::iter(&replaced, 19)? {
        let name = env_name(&r, &pre, &[(20, "<module>"), (7, "env")])?;
        o.line(&format!(
            "replaced {name} (it was granted under the same name)"
        ));
    }
    if py::truthy(&py::dget(&v, "note", 21)?) {
        o.line(&format!("note     {}", py::s(&py::idx(&v, "note", 22)?)));
    }
    let rh = py::or(py::dget(&v, "rehost", 23)?, Value::Object(Vec::new()));
    if py::dget(&rh, "result", 24)? == py::text("done") {
        o.line(&format!(
            "rehost   done ({})",
            py::s(&py::dget(&rh, "deployment", 25)?)
        ));
    } else if py::dget(&rh, "result", 26)? == py::text("not_running") {
        o.line("rehost   not needed: it is not running");
    } else if py::dget(&rh, "result", 28)? == py::text("failed") {
        o.line(&format!(
            "rehost   FAILED: {}",
            py::s(&py::dget(&rh, "error", 29)?)
        ));
    }
    let switches = py::or(py::dget(&v, "switchesTurnedOn", 30)?, py::empty_list());
    for s in py::iter(&switches, 30)? {
        o.line(&format!("switch   {} turned on", py::s(&s)));
    }
    let ok = py::dget(&c, "ok", 32)?;
    if ok == Value::Bool(true) {
        let version = py::or(py::dget(&c, "version", 33)?, py::text("?"));
        let ms = py::dget(&c, "latencyMs", 33)?;
        let from = if py::dget(&c, "from", 33)? == py::text("app") {
            "inside the App"
        } else {
            "the daemon"
        };
        o.line(&format!(
            "check    PASS \u{b7} {} \u{b7} {} ms \u{b7} from {from}",
            py::s(&version),
            py::s(&ms)
        ));
        return Ok(Exit::Ok);
    }
    if py::dget(&c, "ok", 35)? == Value::Null {
        let mut why = py::dget(&c, "note", 36)?;
        if !py::truthy(&why) {
            why = py::or(py::dget(&c, "skipped", 36)?, py::text("not run"));
        }
        o.line(&format!("check    skipped: {}", py::s(&why)));
        return Ok(Exit::Ok);
    }
    let step = py::dget(&c, "step", 38)?;
    let error = py::dget(&c, "error", 38)?;
    o.line(&format!(
        "check    FAIL \u{b7} {}: {}",
        py::s(&step),
        py::s(&error)
    ));
    if py::truthy(&py::dget(&c, "hint", 39)?) {
        o.line(&format!("fix      {}", py::s(&py::idx(&c, "hint", 40)?)));
    }
    let output = py::or(py::dget(&c, "appOutput", 41)?, py::empty_list());
    if py::truthy(&output) {
        let n = py_len(&output, 43)?;
        o.line(&format!("App output (last {n} lines, untrusted):"));
        for l in py::iter(&output, 44)? {
            o.line(&py::concat("  | ", &l, 45)?);
        }
    }
    Ok(Exit::Error)
}

/// `_app_db_held_print`'s Python.
pub fn held_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let r = py::or(py::dget(&v, "request", 4)?, Value::Object(Vec::new()));
    o.line(&format!(
        "held for the owner: {} (one card)",
        py::s(&py::dget(&r, "id", 5)?)
    ));
    let mut summary = py::dget(&r, "summary", 6)?;
    if !py::truthy(&summary) {
        let diff = py::dget_or(&v, "diff", Value::Object(Vec::new()), 6)?;
        let lines = py::dget_or(&diff, "lines", Value::Array(vec![py::text("")]), 6)?;
        summary = super::walls::first(&lines, 6)?;
    }
    o.line(&format!("  {}", py::s(&summary)));
    let switches = py::or(py::dget(&v, "switches", 7)?, py::empty_list());
    for s in py::iter(&switches, 7)? {
        if !py::truthy(&py::dget(&s, "on", 8)?) {
            let name = py::dget(&s, "name", 9)?;
            let at = py::dget(&s, "where", 9)?;
            o.line(&format!(
                "  switch off: \"{}\" ({})",
                py::s(&name),
                py::s(&at)
            ));
        }
    }
    let diff = py::or(py::dget(&v, "diff", 10)?, Value::Object(Vec::new()));
    let lines = py::or(py::dget(&diff, "lines", 10)?, py::empty_list());
    for l in py::iter(&lines, 10)? {
        if py::contains(&l, "belongs to workspace", 11)? {
            o.line(&py::concat("  ", &l, 12)?);
        }
    }
    if py::truthy(&py::dget(&v, "reused", 13)?) {
        o.line(&format!(
            "  this is your earlier card {}, still waiting; nothing new was filed",
            py::s(&py::dget(&r, "id", 14)?)
        ));
    }
    // 0.46.17: where the permission card is and who it went to.
    let card = py::dget(&v, "card", 15)?;
    if matches!(card, Value::Object(_)) && py::truthy(&py::dget(&card, "where", 16)?) {
        o.line(&format!(
            "  owner: {}",
            py::s(&py::idx(&card, "where", 17)?)
        ));
    }
    let hint = py::or(py::dget(&v, "hint", 18)?, py::text(""));
    o.line(&py::concat("  ", &hint, 18)?);
    Ok(Exit::Ok)
}

/// `disconnect`'s Python: exit 1 when the answer names an error.
pub fn disconnect_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let (app, db, r#as) = (
        py::idx(&v, "app", 4)?,
        py::idx(&v, "db", 4)?,
        py::idx(&v, "as", 4)?,
    );
    o.line(&format!(
        "disconnected {} from {} ({})",
        py::s(&app),
        py::s(&db),
        py::s(&r#as)
    ));
    let roles = py::or(py::dget(&v, "roles", 5)?, py::empty_list());
    let roles = py::join(", ", &roles, 5)?;
    let cut = py::dget_or(&v, "sessionsCut", Value::Int("0".into()), 5)?;
    let dropped = if py::truthy(&py::dget(&v, "rolesDropped", 5)?) {
        "; roles dropped"
    } else {
        ""
    };
    o.line(&format!(
        "login    {roles} stopped; {} open session(s) cut{dropped}",
        py::s(&cut)
    ));
    let door = if !py::truthy(&py::dget(&v, "doorOpen", 6)?) {
        "SHUT"
    } else {
        "still open (another connection uses it)"
    };
    o.line(&format!("door     {door}"));
    let secrets = py::or(py::dget(&v, "secrets", 7)?, py::empty_list());
    let mut names = Vec::new();
    for s in py::iter(&secrets, 7)? {
        match &s {
            Value::Str(_) => {
                let t = py::s(&s);
                names.push(t.rsplit('.').next().unwrap_or("").to_string());
            }
            other => {
                return Err(Raise {
                    frames: vec![(7, "<module>"), (7, "<genexpr>")],
                    exception: format!(
                        "AttributeError: '{}' object has no attribute 'split'",
                        pyfmt::type_name(other)
                    ),
                })
            }
        }
    }
    let names = names.join(", ");
    o.line(&format!(
        "secrets  removed: {}",
        if names.is_empty() { "none" } else { &names }
    ));
    o.line(&format!(
        "note     {}",
        py::s(&py::dget_or(&v, "note", py::text(""), 8)?)
    ));
    let e = py::dget(&v, "error", 9)?;
    if py::truthy(&e) {
        o.err(&format!("error    {}", py::s(&py::dget(&e, "hint", 11)?)));
        return Ok(Exit::Error);
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_read_as_the_shell_reads_them() {
        let w = words(
            "connect",
            &[
                "acme",
                "--new",
                "--major=17",
                "--timeout",
                "1h30m",
                "--wait",
            ],
        )
        .unwrap();
        assert_eq!(w.timeout, 5400);
        assert!(w.new && w.wait);
        assert_eq!(w.major, "17");
        assert_eq!(
            words("check", &["a", "--timeout", "007"]).unwrap().timeout,
            7
        );
        let bad = words("check", &[]).unwrap_err();
        assert_eq!(bad.exit, Exit::Error);
        assert!(bad.line.contains("app db check <app-id>"), "{}", bad.line);
        assert_eq!(
            words("check", &["a", "--timeout", "x"]).unwrap_err().exit,
            Exit::Usage
        );
        assert_eq!(parse(&["connect", "a", "--as", "-h"]), None);
        assert_eq!(parse(&["status", "a"]), None);
    }

    #[test]
    fn a_connect_answer_prints_as_the_shell_does() {
        let mut o = Out::default();
        let r = print_lines(
            br#"{"app":"acme","created":true,"db":"acme_main","major":17,"cluster":"main","roles":["l","o"],"secrets":["app-db.acme.DATABASE_URL","other"],"kind":"container","doorOpen":true,"replaced":["app-db.acme.OLD"],"rehost":{"result":"done","deployment":"d1"},"switchesTurnedOn":["s"],"check":{"ok":false,"step":"login","error":"e","hint":"h","appOutput":["a","b"]}}"#,
            &mut o,
        );
        assert_eq!(r.unwrap(), Exit::Error);
        assert_eq!(
            o.stdout,
            "connected acme \u{2192} acme_main (Postgres 17, cluster main)  [new]\nroles    l, o\nsecrets  DATABASE_URL, other (granted to the App; not shown)\ndoor     open (container door, in-wall 127.0.0.1:5432)\nreplaced OLD (it was granted under the same name)\nrehost   done (d1)\nswitch   s turned on\ncheck    FAIL \u{b7} login: e\nfix      h\nApp output (last 2 lines, untrusted):\n  | a\n  | b\n"
        );
        let e = print_lines(br#"{"secrets":[1]}"#, &mut Out::default()).unwrap_err();
        assert_eq!(
            e.frames,
            vec![(14, "<module>"), (14, "<genexpr>"), (7, "env")]
        );
    }
}
