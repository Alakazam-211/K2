//! `_hostchat_print <verb>` (`cli/src/hostchat-voice.sh`): the summary
//! lines of an answer, as its Python prints them, or where it raised.
//! `--json` never comes here: the caller prints the answer as it came.

use super::super::voice::sew;
use crate::families::{get, ljust, py_iter, raised, Raise, Shown};
use crate::out::json::{self, Text, Value};
use crate::out::py;
use crate::out::pylib::ops;
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};

/// The line of `json.load(sys.stdin)` in the Python.
pub const LOAD: u32 = 3;

pub(super) type R<T> = Result<T, Raise>;

/// What the Python printed so far.
#[derive(Default)]
pub(super) struct Py {
    pub out: String,
    pub err: String,
}

impl Py {
    pub fn print(&mut self, t: &str) {
        self.out.push_str(t);
        self.out.push('\n');
    }

    pub fn eprint(&mut self, t: &str) {
        self.err.push_str(t);
        self.err.push('\n');
    }

    /// `line(k, x)`: `"%-16s %s" % (k + ":", x)`, unless `x` is `None`,
    /// `""` or `[]`.
    pub fn line(&mut self, k: &str, x: &Value) {
        let skip = match x {
            Value::Null => true,
            Value::Str(t) => t.code_points().is_empty(),
            Value::Array(a) => a.is_empty(),
            _ => false,
        };
        if !skip {
            self.print(&format!("{} {}", ljust(&format!("{k}:"), 16), sv(x)));
        }
    }

    pub fn line_s(&mut self, k: &str, x: &str) {
        self.line(k, &st(x));
    }
}

pub(super) fn st(t: &str) -> Value {
    Value::Str(Text::from(t))
}

pub(super) fn sv(v: &Value) -> String {
    py::str_of(v)
}

pub(super) fn t(v: &Value) -> bool {
    py::truthy(v)
}

pub(super) fn is(v: &Value, text: &str) -> bool {
    matches!(v, Value::Str(s) if s.as_str() == Some(text))
}

pub(super) fn dict() -> Value {
    Value::Object(Vec::new())
}

/// `v.get(k)`, or the `AttributeError` at `line`.
pub(super) fn g(v: &Value, k: &str, line: u32) -> R<Value> {
    gd(v, k, Value::Null, line)
}

/// `v.get(k, d)`.
pub(super) fn gd(v: &Value, k: &str, d: Value, line: u32) -> R<Value> {
    match v {
        Value::Object(pairs) => Ok(get(pairs, k).cloned().unwrap_or(d)),
        other => Err(Raise::no_attribute(line, other, "get")),
    }
}

/// `v[k]`.
pub(super) fn ix(v: &Value, k: &str, line: u32) -> R<Value> {
    ops::index(v, k, line).cloned()
}

/// `a or b`.
pub(super) fn or(a: Value, b: Value) -> Value {
    if t(&a) {
        a
    } else {
        b
    }
}

/// `(v.get(k) or {})`, or the raise.
pub(super) fn sub(v: &Value, k: &str, line: u32) -> R<Value> {
    Ok(or(g(v, k, line)?, dict()))
}

/// `a + b` for two values Python must find strings.
pub(super) fn plus(a: &Value, b: &Value, line: u32) -> R<String> {
    match (a, b) {
        (Value::Str(_), Value::Str(_)) => Ok(format!("{}{}", sv(a), sv(b))),
        (Value::Str(_), other) => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
        (Value::Array(_), other) => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate list (not \"{}\") to list",
                py::type_name(other)
            ),
        )),
        (x, y) => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and '{}'",
                py::type_name(x),
                py::type_name(y)
            ),
        )),
    }
}

/// `"<head>" + v`.
pub(super) fn cat(head: &str, v: &Value, line: u32) -> R<String> {
    plus(&st(head), v, line)
}

/// `v + "<tail>"`.
pub(super) fn cat_r(v: &Value, tail: &str, line: u32) -> R<String> {
    plus(v, &st(tail), line)
}

/// `sorted(v)`: the keys of a dict, by code point; anything else raises
/// (no caller's value is ever a list of strings here).
fn sorted_keys(v: &Value, line: u32) -> R<Vec<String>> {
    match v {
        Value::Object(pairs) => {
            let mut keys: Vec<(Vec<u32>, String)> = pairs
                .iter()
                .map(|(k, _)| (k.code_points(), sv(&Value::Str(k.clone()))))
                .collect();
            keys.sort();
            keys.dedup();
            Ok(keys.into_iter().map(|(_, k)| k).collect())
        }
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// `for x in (v or [])`.
fn each(v: Value, line: u32) -> R<Vec<Value>> {
    if t(&v) {
        py_iter(&v, line)
    } else {
        Ok(Vec::new())
    }
}

/// `_hostchat_print <verb>` on the answer's text.
pub fn print(raw: &str, verb: &str) -> Shown {
    let mut p = Py::default();
    match print_into(&mut p, raw, verb) {
        Ok(()) => Shown {
            stdout: p.out,
            stderr: p.err,
            exit: crate::exit::Exit::Ok,
        },
        Err(e) => raised(p.out, p.err, &e),
    }
}

/// `json.load(sys.stdin)` at `line`.
pub(super) fn load(raw: &str, line: u32) -> R<Value> {
    json::parse(raw).map_err(|_| {
        Raise::at(
            line,
            "json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)",
        )
    })
}

fn print_into(p: &mut Py, raw: &str, verb: &str) -> R<()> {
    let v = load(raw, LOAD)?;
    match verb {
        "status" => status(p, &v),
        "recording-status" => recording_status(p, &v),
        "recording-install" | "recording-enable" | "recording-disable" | "recording-uninstall" => {
            recording_change(p, &v, verb)
        }
        "preflight" | "media-preflight" | "recording-preflight" => {
            p.print(if t(&g(&v, "ready", 146)?) {
                "ready"
            } else {
                "NOT ready"
            });
            for c in each(g(&v, "checks", 147)?, 147)? {
                let hint = g(&c, "hint", 148)?;
                let tail = if t(&hint) {
                    format!("{})", cat("  (", &ix(&c, "hint", 148)?, 148)?)
                } else {
                    String::new()
                };
                p.print(&format!(
                    "  {} {} {}{}",
                    ljust(&sv(&g(&c, "status", 148)?), 5),
                    ljust(&sv(&g(&c, "id", 148)?), 14),
                    sv(&g(&c, "detail", 148)?),
                    tail
                ));
            }
            Ok(())
        }
        "users-list" => {
            let rows = or(g(&v, "users", 159)?, Value::Array(Vec::new()));
            if !t(&rows) {
                p.print("(no users)");
            }
            for u in py_iter(&rows, 162)? {
                let name = or(g(&u, "username", 163)?, g(&u, "externalId", 163)?);
                let deact = if t(&g(&u, "deactivated", 164)?) {
                    "  (deactivated)"
                } else {
                    ""
                };
                p.print(&format!(
                    "{} {} {} {}{}",
                    ljust(&sv(&name), 24),
                    ljust(&sv(&g(&u, "installRole", 163)?), 8),
                    ljust(&sv(&g(&u, "kind", 163)?), 6),
                    sv(&or(g(&u, "displayName", 164)?, st(""))),
                    deact
                ));
            }
            Ok(())
        }
        v_ if v_.starts_with("users-") => {
            p.line("user", &g(&v, "username", 150)?);
            p.line("role", &g(&v, "installRole", 151)?);
            let sr = g(&v, "sessionsRevoked", 152)?;
            if !matches!(sr, Value::Null) {
                p.line("sessions ended", &sr);
            }
            let sl = sub(&v, "setupLink", 154)?;
            if t(&g(&sl, "url", 155)?) {
                p.print(&format!(
                    "Setup link (one use, until {}; share it privately):",
                    sv(&g(&sl, "expiresAt", 156)?)
                ));
                p.print(&sv(&ix(&sl, "url", 157)?));
            }
            Ok(())
        }
        "app-spaces" => app_spaces(p, &v),
        "app-space-governance" => {
            let gov = g(&v, "governance", 186)?;
            p.print(&format!(
                "Space {}: governed by {} ({})",
                sv(&g(&v, "spaceId", 186)?),
                sv(&gov),
                if t(&g(&v, "changed", 186)?) {
                    "changed"
                } else {
                    "no change"
                }
            ));
            if t(&g(&v, "hint", 187)?) {
                p.print(&sv(&ix(&v, "hint", 188)?));
            }
            Ok(())
        }
        "rollback" => rollback(p, &v),
        "ship" => {
            let sh = sub(&v, "ship", 211)?;
            let bb = g(&sh, "boxBackup", 212)?;
            if !matches!(bb, Value::Null) {
                p.line("off-box copy", &g(&bb, "line", 214)?);
                p.line("covered by", &g(&bb, "coveredBy", 215)?);
                p.line("timer", &g(&bb, "timer", 216)?);
            } else if is(&g(&sh, "kind", 218)?, "dir") {
                p.line("off-box copy", &g(&sh, "path", 218)?);
            } else {
                p.line_s("off-box copy", "none (backups stay on this box)");
            }
            let mi = sub(&v, "mirror", 219)?;
            let path = g(&mi, "path", 220)?;
            let tail = if t(&path) {
                cat(" ", &ix(&mi, "path", 220)?, 220)?
            } else {
                String::new()
            };
            p.line_s(
                "key mirror",
                &format!("{}{}", sv(&g(&mi, "state", 220)?), tail),
            );
            if t(&g(&mi, "error", 221)?) {
                p.line_s(
                    "key mirror",
                    &format!("waiting: {}", sv(&g(&mi, "error", 222)?)),
                );
            }
            p.line("note", &g(&v, "note", 223)?);
            Ok(())
        }
        "adopt" => {
            p.line("Chat / Voice Hosting", &g(&v, "state", 225)?);
            let iv = sub(&v, "installedVersion", 226)?;
            let (from, to) = (g(&iv, "from", 227)?, g(&iv, "to", 227)?);
            if ops::py_eq(&from, &to) {
                p.line("version", &to);
            } else {
                p.line_s(
                    "version",
                    &format!("{} (record said {})", sv(&to), sv(&from)),
                );
            }
            p.line("cleared error", &g(&v, "clearedError", 228)?);
            for a in each(g(&v, "alarms", 229)?, 229)? {
                p.line("alarm", &a);
            }
            Ok(())
        }
        "recovery-key" => {
            let k = or(or(g(&v, "recoveryKey", 232)?, g(&v, "key", 232)?), st(""));
            p.print(&sv(&k));
            if t(&g(&v, "warning", 233)?) {
                p.eprint(&sv(&ix(&v, "warning", 234)?));
            }
            Ok(())
        }
        _ => {
            let Value::Object(pairs) = &v else {
                return Err(Raise::no_attribute(236, &v, "items"));
            };
            let mut keep: Vec<(Text, Value)> = Vec::new();
            for (k, x) in pairs {
                if k.as_str() == Some("ok") {
                    continue;
                }
                match keep.iter_mut().find(|(k2, _)| k2 == k) {
                    Some(slot) => slot.1 = x.clone(),
                    None => keep.push((k.clone(), x.clone())),
                }
            }
            for (k, x) in keep {
                let x = match x {
                    Value::Object(_) | Value::Array(_) => st(&sew::dumps_default(&x)),
                    x => x,
                };
                p.line(&sv(&Value::Str(k)), &x);
            }
            Ok(())
        }
    }
}

fn status(p: &mut Py, v: &Value) -> R<()> {
    let released = g(v, "released", 42)?;
    if matches!(released, Value::Bool(false)) && is(&g(v, "state", 42)?, "not-installed") {
        let n = or(
            g(v, "notice", 43)?,
            st(&format!("not available in this {PRODUCT_NAME} release yet")),
        );
        p.print(&cat("Chat / Voice Hosting: ", &n, 43)?);
        return Ok(());
    }
    if !t(&gd(v, "supported", Value::Bool(true), 44)?) {
        p.print("Chat / Voice Hosting: not supported here (Linux boxes only)");
        return Ok(());
    }
    p.line("Chat / Voice Hosting", &g(v, "state", 46)?);
    if matches!(released, Value::Bool(false)) {
        p.line("notice", &g(v, "notice", 48)?);
    }
    p.line("identity", &g(v, "identityProvider", 49)?);
    let iv = g(v, "installedVersion", 50)?;
    if t(&iv) {
        p.line("version", &iv);
    } else {
        p.line_s(
            "version",
            &format!("pin {}", sv(&g(v, "pinnedVersion", 50)?)),
        );
    }
    p.line("install id", &g(v, "installId", 51)?);
    p.line("public URL", &g(v, "publicUrl", 52)?);
    p.line("app URL", &g(v, "appUrl", 53)?);
    let owner = g(v, "owner", 54)?;
    if !matches!(owner, Value::Null) && !is(&owner, "none") {
        p.line("owner in Sew", &g(v, "owner", 55)?);
    }
    let sh = sub(&sub(v, "backup", 56)?, "ship", 56)?;
    let kind = g(&sh, "kind", 57)?;
    if is(&kind, "box-backup") {
        let line = or(
            g(&sub(&sh, "boxBackup", 58)?, "line", 58)?,
            st("the box backup (k2-backup)"),
        );
        p.line("off-box copy", &line);
    } else if t(&kind) {
        if is(&g(&sh, "kind", 60)?, "dir") {
            p.line("off-box copy", &g(&sh, "path", 60)?);
        } else {
            p.line_s("off-box copy", "none (backups stay on this box)");
        }
        p.line("last copied", &g(&sh, "lastShippedId", 61)?);
        p.line("copy error", &g(&sh, "lastError", 62)?);
    }
    let mi = sub(v, "mirror", 63)?;
    if t(&g(&mi, "kind", 64)?) {
        if is(&g(&mi, "kind", 65)?, "dir") {
            let tail = if t(&g(&mi, "tied", 65)?) {
                " (follows the off-box folder)"
            } else {
                ""
            };
            let path = g(&mi, "path", 65)?;
            p.line_s("key mirror", &cat_r(&path, tail, 65)?);
        } else {
            p.line_s("key mirror", "none");
        }
    }
    let tie = sub(&mi, "tie", 66)?;
    if is(&g(&tie, "state", 67)?, "pending") {
        let why = or(
            g(&tie, "error", 68)?,
            st(&format!("{PRODUCT_NAME} retries on its own")),
        );
        p.line_s("key mirror", &format!("waiting: {}", sv(&why)));
    }
    if t(&g(v, "recoveryKeyInVault", 69)?) {
        let lk = or(g(v, "recoveryKeyVaultName", 70)?, st("sew-launch-key"));
        p.line_s(
            "launch key",
            &format!(
                "in the {PRODUCT_NAME} vault as {} (the owner only); to see it: {CLI_NAME} vault reveal {} --scope engine:sew",
                sv(&lk),
                sv(&lk)
            ),
        );
    }
    let mo = sub(v, "moveOffK2", 72)?;
    if t(&g(&mo, "line", 73)?) {
        p.line(&format!("move off {PRODUCT_NAME}"), &g(&mo, "line", 74)?);
    }
    let au = sub(v, "autoSetup", 75)?;
    if t(&g(&au, "line", 76)?) {
        p.line("set up", &g(&au, "line", 77)?);
    }
    let mt = sub(v, "meet", 78)?;
    if t(&g(&mt, "app", 79)?) {
        p.line_s(
            "meet",
            &format!(
                "{} \u{2192} App {}",
                sv(&g(&mt, "url", 80)?),
                sv(&g(&mt, "app", 80)?)
            ),
        );
    }
    if t(&g(v, "upgradeAvailable", 81)?) {
        p.line_s(
            "upgrade",
            &format!("available: {CLI_NAME} hostchat upgrade"),
        );
    }
    let alarms = or(g(v, "alarms", 83)?, Value::Array(Vec::new()));
    p.line_s("alarms", &ops::join(&alarms, ", ", 83)?);
    let pgs = sub(v, "sewDbPostgres", 84)?;
    if t(&g(&pgs, "stale", 85)?) {
        let iv = g(&pgs, "installedVersion", 87)?;
        let ver = if t(&iv) {
            cat(" ", &ix(&pgs, "installedVersion", 87)?, 87)?
        } else {
            String::new()
        };
        p.line_s(
            "sew-db",
            &format!(
                "runs an older Postgres than the installed {}{}. {}",
                sv(&g(&pgs, "package", 87)?),
                ver,
                sv(&g(&pgs, "fix", 87)?)
            ),
        );
    }
    for w in each(g(v, "warnings", 88)?, 88)? {
        let m = or(g(&w, "message", 89)?, g(&w, "code", 89)?);
        p.line("warning", &m);
    }
    p.line("last error", &g(v, "lastError", 90)?);
    let rc = sub(v, "recording", 91)?;
    if t(&g(&rc, "installed", 92)?) {
        let on = if t(&g(&rc, "enabled", 93)?) {
            "on"
        } else {
            "off, recorder kept"
        };
        let ver = or(g(&rc, "version", 93)?, st("?"));
        p.line_s("recording", &format!("{on} (recorder {})", sv(&ver)));
    }
    let vo = sub(v, "voice", 94)?;
    if t(&vo) {
        p.line("voice", &g(&vo, "state", 96)?);
        p.line("voice URL", &g(&vo, "publicUrl", 97)?);
        let turn = g(&vo, "turnHost", 98)?;
        if t(&turn) {
            p.line_s(
                "TURN",
                &format!("{}:5349 (node {})", sv(&turn), sv(&g(&vo, "nodeIp", 98)?)),
            );
        }
        let c = sub(&vo, "cert", 99)?;
        let dl = g(&c, "daysLeft", 100)?;
        if !matches!(dl, Value::Null) {
            p.line_s("TURN cert", &format!("{} days left", sv(&dl)));
        }
        let tls = sub(&vo, "turnTls", 102)?;
        p.line("TURN-TLS check", &g(&tls, "served", 103)?);
        let pr = sub(&vo, "progress", 104)?;
        let res = g(&pr, "result", 105)?;
        if is(&res, "running") || is(&res, "error") {
            p.line_s(
                "voice step",
                &format!("{} ({})", sv(&g(&pr, "step", 106)?), sv(&res)),
            );
        }
        p.line("voice error", &g(&vo, "lastError", 107)?);
        let ex = sub(&vo, "exposure", 108)?;
        if t(&g(&ex, "note", 109)?) {
            p.line("exposure", &g(&ex, "note", 110)?);
        }
    }
    Ok(())
}

fn recording_status(p: &mut Py, v: &Value) -> R<()> {
    let state = if t(&g(v, "installed", 112)?) {
        if t(&g(v, "enabled", 112)?) {
            "on"
        } else {
            "off"
        }
    } else {
        "not installed"
    };
    p.line_s("recording", state);
    let ver = sub(v, "version", 113)?;
    p.line("recorder", &g(&ver, "recorder", 114)?);
    p.line("transcriber", &g(&ver, "transcriber", 115)?);
    p.line("model", &g(v, "model", 116)?);
    let pol = sub(v, "policies", 117)?;
    let mut parts = Vec::new();
    for k in sorted_keys(&pol, 118)? {
        let x = ix(&pol, &k, 118)?;
        parts.push(format!("{k}={}", sew::dumps_default(&x)));
    }
    p.line_s("policies", &parts.join(", "));
    let fl = sub(v, "flags", 119)?;
    let mut on = Vec::new();
    for k in sorted_keys(&fl, 120)? {
        if t(&ix(&fl, &k, 120)?) {
            on.push(k);
        }
    }
    let on = on.join(", ");
    p.line_s(
        "Sew flags",
        if on.is_empty() {
            "none of the Sew 0.9.0 recording flags"
        } else {
            &on
        },
    );
    let pr = sub(v, "progress", 121)?;
    let res = g(&pr, "result", 122)?;
    if is(&res, "running") || is(&res, "error") {
        p.line_s(
            "step",
            &format!("{} ({})", sv(&g(&pr, "step", 123)?), sv(&res)),
        );
    }
    p.line("last error", &g(v, "lastError", 124)?);
    let r = sub(v, "ready", 125)?;
    if matches!(g(&r, "ok", 126)?, Value::Bool(false)) {
        p.line_s(
            "not ready",
            &format!("{}: {}", sv(&g(&r, "code", 127)?), sv(&g(&r, "hint", 127)?)),
        );
    }
    Ok(())
}

fn recording_change(p: &mut Py, v: &Value, verb: &str) -> R<()> {
    let installing = is(&g(v, "state", 130)?, "installing");
    let busy = format!("installing ({CLI_NAME} hostchat recording status)");
    match verb {
        "recording-install" => {
            p.line_s("recorder", if installing { &busy } else { "installed" });
            let on = if t(&g(v, "enabled", 131)?) {
                "on"
            } else {
                "off"
            };
            p.line_s("recording", on);
            p.line("version", &g(v, "version", 132)?);
            p.line("model", &g(v, "model", 133)?);
            p.line("next", &g(v, "next", 134)?);
        }
        "recording-enable" => {
            p.line_s("recording", if installing { &busy } else { "on" });
            p.line("version", &g(v, "version", 137)?);
            p.line("model", &g(v, "model", 138)?);
            p.line("next", &g(v, "next", 139)?);
        }
        _ => {
            let gone = if t(&g(v, "installed", 141)?) {
                "off"
            } else {
                "removed"
            };
            p.line_s("recording", gone);
            p.line("kept", &g(v, "kept", 142)?);
            for w in each(g(v, "warnings", 143)?, 143)? {
                p.line("warning", &w);
            }
        }
    }
    Ok(())
}

fn app_spaces(p: &mut Py, v: &Value) -> R<()> {
    let rows = or(g(v, "spaces", 166)?, Value::Array(Vec::new()));
    if !t(&rows) {
        p.print("(no App Spaces bound)");
    }
    for r in py_iter(&rows, 169)? {
        let m = sub(&r, "members", 170)?;
        let mut gov = String::new();
        if t(&g(&r, "governance", 172)?) {
            let conf = g(&r, "governanceConfirmed", 173)?;
            let want = ix(&r, "governance", 174)?;
            gov = format!(
                "; governed by: {}{}",
                sv(&want),
                if ops::py_eq(&conf, &want) {
                    ""
                } else {
                    " (sending to Sew)"
                }
            );
        }
        let n = |k: &str| gd(&m, k, Value::Int("0".into()), 175);
        p.print(&format!(
            "{} {} added {}, held {}, pending {}{}",
            ljust(&sv(&g(&r, "appId", 175)?), 24),
            ljust(&sv(&g(&r, "spaceId", 175)?), 28),
            sv(&n("added")?),
            sv(&n("held")?),
            sv(&n("pending")?),
            gov
        ));
    }
    let gone = or(g(v, "unbound", 176)?, Value::Array(Vec::new()));
    if t(&gone) {
        p.print("Unbound Spaces (an App removed, or app-space unbind; Sew keeps each one read-only for its old App; history and export still work;");
        p.print("  to reuse one for another App, the Sew install owner releases it in Sew first):");
    }
    for r in py_iter(&gone, 180)? {
        let state = if t(&g(&r, "doneAt", 181)?) {
            "read-only in Sew"
        } else {
            "waiting: sent once Sew can unbind (Sew 0.5.0+)"
        };
        p.print(&format!(
            "  {} {} {}",
            ljust(&sv(&g(&r, "appId", 182)?), 24),
            ljust(&sv(&g(&r, "spaceId", 182)?), 28),
            state
        ));
    }
    if t(&g(v, "note", 183)?) {
        p.eprint(&sv(&ix(v, "note", 184)?));
    }
    Ok(())
}

fn rollback(p: &mut Py, v: &Value) -> R<()> {
    p.line("Chat / Voice Hosting", &g(v, "state", 190)?);
    p.line("version", &g(v, "version", 191)?);
    let m = sub(v, "media", 192)?;
    let state = g(&m, "state", 193)?;
    if t(&state) {
        let (from, to) = (g(&m, "from", 195)?, g(&m, "to", 195)?);
        let tail = if t(&from) && t(&to) {
            format!(" ({} -> {})", sv(&from), sv(&to))
        } else {
            String::new()
        };
        p.line_s("voice engine", &cat_r(&state, &tail, 195)?);
    } else if t(&g(&m, "result", 196)?) {
        p.line_s(
            "voice engine",
            &format!(
                "{} (sew-media {})",
                sv(&g(&m, "result", 197)?),
                sv(&g(&m, "to", 197)?)
            ),
        );
    }
    if t(&g(v, "mediaCommand", 198)?) {
        p.print("Voice engine NOT rolled back. Run this once on the box, as root:");
        p.print(&cat("  ", &ix(v, "mediaCommand", 200)?, 200)?);
        p.print(&format!(
            "The rollback itself is done: do not run {CLI_NAME} hostchat rollback again."
        ));
    } else if t(&g(v, "mediaNote", 202)?) {
        p.print(&sv(&ix(v, "mediaNote", 203)?));
    }
    for w in each(g(v, "warnings", 204)?, 204)? {
        p.line("warning", &w);
    }
    let d = sub(v, "doctor", 206)?;
    if t(&d) {
        p.line_s(
            "sew doctor",
            if t(&g(&d, "ok", 208)?) {
                "voice engine ok"
            } else {
                "voice engine checks fail (see --json)"
            },
        );
    }
    p.line("error", &g(v, "error", 209)?);
    Ok(())
}
