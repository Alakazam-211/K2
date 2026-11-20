//! `k5 users pair-phone`, `phones` and `unpair-phone`, as the shell's
//! `_users_pair_phone`, `_users_phones` and `_users_unpair_phone`
//! (`cli/src/users-phones.sh`): a POST of `/cli/auth/device/pair-code`,
//! a GET of `/cli/auth/devices` and a POST of `/cli/auth/device/revoke`.
//! Each answer goes through the shared reader's `fail` ([`fail`]); a name
//! from a phone is printed with each character Python can't print as `?`
//! ([`safe`]).
//!
//! `pair-phone` from a login (not the owner token) is answered "type your
//! password again": the password is asked (no echo on a terminal, else one
//! line of stdin) and the code asked for again with it in the body.

use super::read::{loads, or, text, unparseable};
use super::{s, Stop, Users};
use crate::cards::Decode;
use crate::exit::Exit;
use crate::families::tunnel::Out;
use crate::families::{get, ljust, py_get, py_iter, py_len, py_space, raised, Raise, Shown};
use crate::out::json::{self, Options, Value};
use crate::out::{self, py};
use crate::prompt::{self, Secret};
use crate::transport::Call;
use k2_client::{cli_name, product_name};

pub const PAIR_CODE: &str = "/cli/auth/device/pair-code";
pub const DEVICES: &str = "/cli/auth/devices";
pub const REVOKE: &str = "/cli/auth/device/revoke";

/// The daemon's words when a login must type its password again.
pub const AGAIN: &[u8] = b"type your password again to make a pairing code";
pub const PROMPT: &str = "Password for your login: ";
pub const NEED_PASSWORD: &str =
    "Error: a password is needed to make a pairing code for your login.";
pub const EMPTY: &str = "Error: empty response from daemon (is it running?)";

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    Pair {
        user: String,
        json: bool,
        invert: bool,
    },
    List {
        json: bool,
    },
    Unpair {
        id: String,
        json: bool,
    },
}

/// The words after `pair-phone`, `phones` or `unpair-phone`.
pub fn parse(verb: &str, rest: &[&str]) -> Result<Form, Stop> {
    let unexpected = |w: &str, usage: &str| Stop {
        lines: vec![
            format!("Error: unexpected argument '{w}'"),
            format!("Usage: {} users {usage}", cli_name!()),
        ],
        page: false,
        exit: Exit::Usage,
    };
    let help = || Stop {
        lines: Vec::new(),
        page: true,
        exit: Exit::Ok,
    };
    let mut json = false;
    match verb {
        "pair-phone" => {
            let (mut user, mut invert) = (String::new(), false);
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i];
                match w {
                    "--user" => {
                        user = rest.get(i + 1).copied().unwrap_or("").to_string();
                        if i + 1 >= rest.len() {
                            return Err(Stop {
                                lines: vec!["Error: --user needs a login".into()],
                                page: false,
                                exit: Exit::Usage,
                            });
                        }
                        i += 1;
                    }
                    "--json" => json = true,
                    "--invert" => invert = true,
                    "--help" | "-h" => return Err(help()),
                    _ if w.starts_with("--user=") => user = w["--user=".len()..].to_string(),
                    _ => return Err(unexpected(w, "pair-phone [--user <login>] [--json] [--invert]")),
                }
                i += 1;
            }
            Ok(Form::Pair { user, json, invert })
        }
        "phones" => {
            for w in rest {
                match *w {
                    "--json" => json = true,
                    "--help" | "-h" => return Err(help()),
                    _ => return Err(unexpected(w, "phones [--json]")),
                }
            }
            Ok(Form::List { json })
        }
        _ => {
            let mut id = String::new();
            for w in rest {
                match *w {
                    "--json" => json = true,
                    "--help" | "-h" => return Err(help()),
                    _ if w.starts_with('-') => {
                        return Err(Stop {
                            lines: vec![format!("Error: unknown flag '{w}'")],
                            page: false,
                            exit: Exit::Usage,
                        })
                    }
                    _ if id.is_empty() => id = w.to_string(),
                    _ => {
                        return Err(Stop {
                            lines: vec![format!("Error: unexpected argument '{w}'")],
                            page: false,
                            exit: Exit::Usage,
                        })
                    }
                }
            }
            if id.is_empty() {
                return Err(Stop {
                    lines: vec![format!(
                        "Usage: {cli} users unpair-phone <id> [--json]   (ids: {cli} users phones)",
                        cli = cli_name!()
                    )],
                    page: false,
                    exit: Exit::Usage,
                });
            }
            Ok(Form::Unpair { id, json })
        }
    }
}

/// The verb's run: its calls, then its reader.
pub fn answer(u: &mut Users<'_>, form: Form, decode: Decode) -> Result<Exit, Exit> {
    let resp = match &form {
        Form::Pair { user, .. } => {
            let body = user_body(user, None);
            let mut resp = u.held(PAIR_CODE, Call::PostJson(body.as_bytes()))?;
            if contains(&resp, AGAIN) {
                let pw = if prompt::stdin_is_tty() {
                    prompt::ask_hidden(PROMPT)
                } else {
                    prompt::read_line(prompt::Echo::Shown, prompt::AtEof::Drop)
                };
                if pw.is_empty() {
                    out::stderr_line(NEED_PASSWORD);
                    return Ok(Exit::Usage);
                }
                if decode == Decode::Strict {
                    if let Err(e) = std::str::from_utf8(pw.expose()) {
                        let at = e.valid_up_to();
                        let r = Raise::at(
                            3,
                            format!(
                                "UnicodeDecodeError: 'utf-8' codec can't decode byte 0x{:02x} in position {at}: invalid start byte",
                                pw.expose()[at]
                            ),
                        );
                        return Ok(raised(String::new(), String::new(), &r).print());
                    }
                }
                let body = user_body(user, Some(&pw));
                drop(pw);
                resp = u.held(PAIR_CODE, Call::PostJson(body.as_bytes()))?;
            }
            resp
        }
        Form::List { .. } => u.held(DEVICES, Call::Request(&[]))?,
        Form::Unpair { id, .. } => {
            let body = crate::confirm::dumps(&crate::confirm::object(vec![("id", s(id))]));
            u.held(REVOKE, Call::PostJson(body.as_bytes()))?
        }
    };
    if resp.is_empty() {
        out::stderr_line(EMPTY);
        return Ok(Exit::Error);
    }
    let shown = match form {
        Form::Pair { json, invert, .. } => read(&resp, |d, o| pair_lines(d, o, json, invert)),
        Form::List { json } => read(&resp, |d, o| list_lines(d, o, json)),
        Form::Unpair { json, .. } => read(&resp, |d, o| unpair_lines(d, o, json)),
    };
    Ok(shown.print())
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// `pair-phone`'s body: the password (from its Python's stdin) first,
/// then the login when one was named.
pub fn user_body(user: &str, pw: Option<&Secret>) -> String {
    let mut b = Vec::new();
    if let Some(pw) = pw {
        b.push(("password", super::pw_value(pw)));
    }
    if !user.is_empty() {
        b.push(("user", s(user)));
    }
    crate::confirm::dumps(&crate::confirm::object(b))
}

/// The shared reader: `fail(raw)` (the daemon's error, exit 3 for a
/// refusal, else 1), then the verb's own lines.
pub fn read(raw: &[u8], lines: impl Fn(&Value, &mut Out) -> Result<(), Raise>) -> Shown {
    let t = text(raw);
    let Some(d) = loads(&t) else {
        return unparseable("Error: daemon returned an unparseable response: ", &t);
    };
    let Value::Object(p) = &d else {
        return Shown::err("Error: unexpected response\n", Exit::Error);
    };
    match get(p, "error").filter(|e| **e != Value::Null) {
        None => {
            let mut o = Out::default();
            let r = lines(&d, &mut o);
            o.done(r.map(|()| Exit::Ok))
        }
        Some(err) => fail(err),
    }
}

const REFUSED: [&str; 10] = [
    "login_required",
    "owner_token_required",
    "owner_login_not_linked",
    "password_invalid",
    "password_change_required",
    "device_pairing_not_from_browser",
    "rate_limited",
    "device_session_ended",
    "no_such_device",
    "role_required",
];

/// `fail`'s error branch: its line, then exit 3 for a refusal, else 1.
fn fail(err: &Value) -> Shown {
    let (code, hint) = match err {
        Value::Object(ep) => {
            let code = or(get(ep, "code"), s(""));
            let hint = or(
                get(ep, "hint"),
                or(get(ep, "message"), or(Some(&code), s("error"))),
            );
            (code, hint)
        }
        other => (s(""), s(&py::str_of(other))),
    };
    let tail = if py::truthy(&code) {
        match super::read::str_plus(" (", &code, 18) {
            Ok(t) => format!("{t})"),
            Err(r) => return raised(String::new(), String::new(), &r),
        }
    } else {
        String::new()
    };
    let line = format!("Error: {}{tail}\n", py::str_of(&hint));
    let named = matches!(&code, Value::Str(c) if c.as_str().is_some_and(|c| REFUSED.contains(&c)));
    if named {
        return Shown::err(line, Exit::Refused);
    }
    match &hint {
        Value::Str(_) => {
            let token = py::str_of(&hint).to_lowercase().contains("token");
            Shown::err(line, if token { Exit::Refused } else { Exit::Error })
        }
        // The line is written before `hint.lower()` raises.
        other => raised(
            String::new(),
            line,
            &Raise::at(
                21,
                format!(
                    "AttributeError: '{}' object has no attribute 'lower'",
                    py::type_name(other)
                ),
            ),
        ),
    }
}

/// `safe(s)`: `str(s or "")` with each character Python can't print as
/// `?`.
pub fn safe(v: Option<&Value>) -> String {
    let text = match v {
        Some(x) if py::truthy(x) => x.clone(),
        _ => s(""),
    };
    let Value::Str(t) = &text else {
        return py::str_of(&text).chars().map(printable_or_mark).collect();
    };
    t.code_points()
        .into_iter()
        .map(|cp| char::from_u32(cp).map_or('?', printable_or_mark))
        .collect()
}

/// Python's `c.isprintable()`: printable ASCII, or what `repr` shows as
/// itself.
fn printable_or_mark(c: char) -> char {
    let ok = if c.is_ascii() {
        (' '..='~').contains(&c)
    } else {
        py::repr(&s(&c.to_string())) == format!("'{c}'")
    };
    if ok {
        c
    } else {
        '?'
    }
}

/// `print(json.dumps(d, ensure_ascii=False))`.
fn dumped(d: &Value, o: &mut Out) {
    let opts = Options {
        ensure_ascii: false,
        ..Options::default()
    };
    o.out(&json::to_string(d, &opts).unwrap_or_default());
}

/// `pair-phone`'s lines: the QR (light modules as blocks; dark ones with
/// `--invert`), then the code, the link and how long it works.
pub fn pair_lines(d: &Value, o: &mut Out, as_json: bool, invert: bool) -> Result<(), Raise> {
    if as_json {
        dumped(d, o);
        return Ok(());
    }
    let qr = or(py_get(d, "qr", 6)?, Value::Object(Vec::new()));
    let rows = or(py_get(&qr, "rows", 7)?, Value::Array(Vec::new()));
    if py::truthy(&rows) {
        let n = py_len(&rows, 12)? as i64;
        let pad = 2i64;
        let dark = |x: i64, y: i64| -> Result<bool, Raise> {
            if x < 0 || y < 0 || x >= n || y >= n {
                return Ok(false);
            }
            let row = index(&rows, y, 17)?;
            Ok(index(&row, x, 17)? == s("1"))
        };
        let ink = |x: i64, y: i64| -> Result<bool, Raise> {
            let d = dark(x, y)?;
            Ok(if invert { d } else { !d })
        };
        let mut lines = Vec::new();
        let mut y = -pad;
        while y < n + pad {
            let mut line = String::new();
            for x in -pad..n + pad {
                let top = ink(x, y)?;
                let bot = if y + 1 < n + pad { ink(x, y + 1)? } else { !invert };
                line.push(match (top, bot) {
                    (true, true) => '\u{2588}',
                    (true, false) => '\u{2580}',
                    (false, true) => '\u{2584}',
                    (false, false) => ' ',
                });
            }
            lines.push(line);
            y += 2;
        }
        o.out(&lines.join("\n"));
        o.out("");
    }
    let Value::Object(p) = d else {
        return Ok(());
    };
    o.out(&format!(
        "Pairing code for {}: {}",
        safe(get(p, "user")),
        safe(get(p, "code"))
    ));
    if get(p, "pairUrl").is_some_and(py::truthy) {
        o.out(&format!("Link: {}", safe(get(p, "pairUrl"))));
        o.out(&format!(
            "Scan the QR with the phone (or open the link) to pair it in the {} app.",
            product_name!()
        ));
    } else {
        o.out(&format!(
            "This server has no public host, so there is no link or QR: type the code on the {} app's Pair screen.",
            product_name!()
        ));
    }
    let secs = int_of(&or(get(p, "expiresInSecs"), Value::Int("600".into())), 44)?;
    o.out(&format!(
        "Works once, for {} minutes (until {}).",
        secs.div_euclid(60),
        safe(get(p, "expiresAt"))
    ));
    o.out("A paired phone stays signed in while used (30 days unused or 180 days in all, then it pairs again).");
    Ok(())
}

/// `v[i]` for a list or a string, with Python's errors.
fn index(v: &Value, i: i64, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Array(items) => usize::try_from(i)
            .ok()
            .and_then(|i| items.get(i).cloned())
            .ok_or_else(|| Raise::at(line, "IndexError: list index out of range")),
        Value::Str(t) => usize::try_from(i)
            .ok()
            .and_then(|i| t.code_points().get(i).copied())
            .map(|cp| Value::Str(crate::families::code_point(cp)))
            .ok_or_else(|| Raise::at(line, "IndexError: string index out of range")),
        Value::Object(pairs) => {
            let _ = pairs;
            Err(Raise::at(line, format!("KeyError: {i}")))
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

/// Python's `int(v)`.
fn int_of(v: &Value, line: u32) -> Result<i128, Raise> {
    let bad = |w: &str| {
        Raise::at(
            line,
            format!(
                "ValueError: invalid literal for int() with base 10: {}",
                py::repr(&s(w))
            ),
        )
    };
    match v {
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Int(d) => d.parse().map_err(|_| bad(d)),
        Value::Float(f) if f.is_finite() => Ok(f.trunc() as i128),
        Value::Float(f) => Err(Raise::at(
            line,
            if f.is_nan() {
                "ValueError: cannot convert float NaN to integer".to_string()
            } else {
                "OverflowError: cannot convert float infinity to integer".to_string()
            },
        )),
        Value::Str(t) => {
            let w = py::str_of(v);
            let _ = t;
            let trimmed = w.trim_matches(py_space).replace('_', "");
            let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(&trimmed);
            if digits.is_empty()
                || !digits.bytes().all(|b| b.is_ascii_digit())
                || w.contains("__")
                || w.trim_matches(py_space).starts_with('_')
                || w.trim_matches(py_space).ends_with('_')
            {
                return Err(bad(&w));
            }
            trimmed.parse().map_err(|_| bad(&w))
        }
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
                py::type_name(other)
            ),
        )),
    }
}

/// `phones`' lines: who a new code pairs as, then the paired phones.
pub fn list_lines(d: &Value, o: &mut Out, as_json: bool) -> Result<(), Raise> {
    if as_json {
        dumped(d, o);
        return Ok(());
    }
    let pa = or(py_get(d, "pairAs", 6)?, Value::Object(Vec::new()));
    if py_get(&pa, "user", 7)?.is_some_and(py::truthy) {
        o.out(&format!(
            "A new pairing code pairs as: {}",
            safe(py_get(&pa, "user", 8)?)
        ));
    } else if let Some(e @ Value::Object(_)) = py_get(&pa, "error", 9)? {
        let hint = or(py_get(e, "hint", 10)?, py_get(e, "code", 10)?.cloned().unwrap_or(Value::Null));
        o.out(&format!("A new pairing code: {}", safe(Some(&hint))));
    }
    let devices = or(py_get(d, "devices", 11)?, Value::Array(Vec::new()));
    if !py::truthy(&devices) {
        o.out("No paired phones.");
        return Ok(());
    }
    let mut rows: Vec<[String; 6]> = vec![[
        "ID", "LOGIN", "PHONE", "PAIRED", "ENDS IF UNUSED", "PAIRED BY",
    ]
    .map(String::from)];
    for x in py_iter(&devices, 16)? {
        let at = |k: &str| py_get(&x, k, 17);
        let mut name = safe(at("name")?);
        if at("current")?.is_some_and(py::truthy) {
            name.push_str("  (this one)");
        }
        let first19 = |t: String| t.chars().take(19).collect::<String>();
        rows.push([
            safe(at("id")?),
            safe(at("username")?),
            name,
            first19(safe(at("createdAt")?)),
            first19(safe(at("expiresAt")?)),
            safe(Some(&or(at("pairedBy")?, s("-")))),
        ]);
    }
    let w: Vec<usize> = (0..6)
        .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0))
        .collect();
    for r in &rows {
        let cells: Vec<String> = r.iter().enumerate().map(|(i, c)| ljust(c, w[i])).collect();
        o.out(cells.join("  ").trim_end_matches(py_space));
    }
    Ok(())
}

/// `unpair-phone`'s line.
pub fn unpair_lines(d: &Value, o: &mut Out, as_json: bool) -> Result<(), Raise> {
    if as_json {
        dumped(d, o);
        return Ok(());
    }
    let Value::Object(p) = d else {
        return Ok(());
    };
    o.out(&format!(
        "Unpaired phone {} ({}). It must pair again to sign in.",
        safe(get(p, "revoked")),
        safe(get(p, "username"))
    ));
    Ok(())
}
