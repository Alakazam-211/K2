//! `k5 calendar link <address> --provider google|microsoft …`: `_cal_py`'s
//! `link` branch (`cli/src/calendar.sh`), line for line.
//!
//! - `--request`: one `call` (with its tries) to
//!   `/cli/mail/calendar/link-request`, a card on the agent's Thread.
//! - Otherwise `call_or_owner` (one plain `urlopen`, no tries) to
//!   `/cli/mail/link/oauth/start`: Google in a browser, Google by
//!   paste-back (`--paste`, `--paste-file`, or when the daemon answers
//!   `remote_unsupported`), or Microsoft's device code. Then the status
//!   every 3 s until it is `connected`, `denied`, `expired` or `error`
//!   (at most 900 s), then `--calendars` and `--omit-description`.
//!
//! The paste-back's redirect port is random (20000 to 59999), as the
//! shell's `secrets.randbelow`.
//!
//! The shell's paste prompt never reads the person's paste: `_cal_py` is
//! read from stdin (`python3 - <<'PYEOF'`), so its `sys.stdin` is the
//! script's heredoc, never a terminal, and `sys.stdin.readline()` gives
//! nothing. So `--paste` with no `--paste-file` prints the prompt and
//! ends with the usage error for an address that isn't Google's. That is
//! a bash bug, reported; the binary does the same until it is fixed.

use super::{at, dict, dumps, is_str, join, obj, s, st, truthy, Exit, Py, Tb, Within, MODULE, R};
use crate::out::json::{self, Text, Value};
use crate::out::pylib::{self, ops::py_in};
use crate::out::{self, py};
use crate::transport::urllib::{self, Method, Outcome, Request};
use k2_client::{cli_name, product_name};
use std::time::{Duration, Instant};

/// `call_or_owner`'s and `must`'s lines (the script's, from stdin).
mod line {
    pub const REQUEST_CALL: u32 = 276;
    pub const MUST_ERROR: u32 = 312;
    pub const START: u32 = 323;
    pub const REMOTE: u32 = 324;
    pub const START_MUST: u32 = 328;
    pub const PASTE_START: u32 = 336;
    pub const PASTE_READ: u32 = 344;
    pub const COMPLETE: u32 = 360;
    pub const COMPLETE_MUST: u32 = 366;
    pub const MS_START: u32 = 368;
    pub const STATUS: u32 = 378;
    pub const STATUS_ERROR: u32 = 379;
    pub const FINAL_CONSENT: u32 = 389;
    pub const FINAL_ADD: u32 = 390;
    pub const CALS: u32 = 395;
    pub const PRIVACY: u32 = 398;
    pub const CALS_ON: u32 = 409;
}

/// `link`'s own settings (`C_PROVIDER`, `C_WORKSPACE`, …).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Link {
    pub provider: String,
    pub workspace: String,
    pub cals: String,
    pub paste_file: String,
    pub omit: bool,
    pub write: bool,
    pub paste: bool,
    pub request: bool,
}

/// The poll's limit (`deadline = time.time() + 900`).
const DEADLINE: Duration = Duration::from_secs(900);
/// `time.sleep(3)` between polls.
const PAUSE: Duration = Duration::from_secs(3);

const PASTE_PROMPT: &str = "3. Paste it here and press Enter (it isn't shown or kept): ";

const NOT_GOOGLE: &str = concat!(
    "that isn't the address Google redirected to (it has code= and state=) — run '",
    cli_name!(),
    " cal link' again"
);
const EXPIRED: &str = concat!(
    "the link expired or the daemon restarted — start again with '",
    cli_name!(),
    " cal link'"
);
const TIMED_OUT: &str = concat!(
    "the link did not complete in time — run '",
    cli_name!(),
    " cal link' again"
);
const NO_BROWSER: &str = concat!(
    "This ",
    product_name!(),
    " can't open a browser here — using paste-back instead.\n"
);

/// `owner_hint`'s words.
fn owner_ask(address: &str, provider: &str) -> String {
    format!(
        "linking a calendar is your human's (owner or Admin) — ask with '{} cal link --request {address} --provider {}'",
        cli_name!(),
        if provider.is_empty() { "google" } else { provider }
    )
}

/// `20000 + secrets.randbelow(40000)`.
fn random_port() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    20000 + (h.finish() % 40000) as u32
}

/// `str.strip()`.
fn strip(t: &str) -> &str {
    t.trim_matches(crate::families::py_space)
}

/// `urllib.parse.unquote(s.replace("+", " "))`: `%XX` escapes as bytes,
/// read as UTF-8 with each bad sequence replaced.
fn unquote_plus(s: &str) -> String {
    let b = s.replace('+', " ").into_bytes();
    let mut out = Vec::with_capacity(b.len());
    let hex = |c: u8| (c as char).to_digit(16);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `parse_qs(urlsplit(pasted).query) if "?" in pasted else {}`, then
/// `(q.get(k) or [""])[0]` for `k`.
pub fn one(pasted: &str, k: &str) -> String {
    if !pasted.contains('?') {
        return String::new();
    }
    // urlsplit: the C0 controls and spaces in front, and every tab and
    // line break, go; the fragment, then the query.
    let url: String = pasted
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let url = url.split_once('#').map_or(url.as_str(), |(u, _)| u);
    let Some((_, query)) = url.split_once('?') else {
        return String::new();
    };
    for part in query.split('&') {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        if unquote_plus(name) == k {
            return unquote_plus(value);
        }
    }
    String::new()
}

impl Py {
    /// `call_or_owner(method, route, body, params, timeout)` from module
    /// line `from`: the status and the answer, as one plain `urlopen`.
    fn call_or_owner(
        &self,
        method: Method,
        route: &str,
        body: Option<&Value>,
        params: Vec<(String, String)>,
        timeout: u64,
        from: u32,
    ) -> R<(u16, Value)> {
        let mut req = Request::new(method, route, Duration::from_secs(timeout)).query(params);
        if let Some(b) = body {
            req = req
                .data(dumps(b).into_bytes())
                .header("Content-Type", "application/json");
        }
        if let Some(t) = self.opener.token() {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        // `_K2NoRedirect`: a redirect's line, then `SystemExit(3)`.
        let out = self
            .opener
            .open_ruled(&req)
            .map_err(|r| super::End::Exit(r.write()))?;
        match out {
            Outcome::Answer {
                status,
                body: Ok(b),
            } => match json::parse(&urllib::decode(&b)) {
                Ok(v) => Ok((status, v)),
                Err(_) => Err(self.unreachable()),
            },
            Outcome::HttpError { code, body: Ok(b) } => Ok((
                code,
                json::parse(&urllib::decode(&b)).unwrap_or_else(|_| {
                    obj(vec![(
                        "error",
                        obj(vec![
                            ("code", s("error")),
                            ("hint", s(&format!("HTTP {code} on {route}"))),
                        ]),
                    )])
                }),
            )),
            // `e.read()` raising inside the handler: out of the run.
            Outcome::HttpError { body: Err(r), .. } => {
                Err(Tb::at(from, MODULE, r.traceback_line()))
            }
            Outcome::Answer { body: Err(_), .. } | Outcome::Raised(_) => Err(self.unreachable()),
        }
    }

    /// `owner_hint(err)`: the ask-your-human words for an owner's refusal.
    fn owner_hint(&self, err: &Value, provider: &str, address: &str) -> Value {
        let code = at(err, "code");
        if !(is_str(code, "forbidden") || is_str(code, "owner_only")) {
            return err.clone();
        }
        let hint = owner_ask(address, provider);
        let Value::Object(mut pairs) = err.clone() else {
            return err.clone();
        };
        match pairs.iter_mut().find(|(k, _)| k.as_str() == Some("hint")) {
            Some(slot) => slot.1 = s(&hint),
            None => pairs.push((Text::from("hint"), s(&hint))),
        }
        Value::Object(pairs)
    }

    /// `must((status, d))` from module line `from`.
    fn must(&self, d: Value, from: u32, provider: &str, address: &str) -> R<Value> {
        dict(&d, line::MUST_ERROR, "must").within(from, MODULE)?;
        if let Some(e @ Value::Object(_)) = at(&d, "error") {
            return Err(self.die(&self.owner_hint(e, provider, address), None));
        }
        if !truthy(at(&d, "ok")) {
            let text: String = dumps(&d).chars().take(200).collect();
            return Err(self.die(
                &obj(vec![
                    ("code", s("error")),
                    ("hint", s(&format!("unexpected daemon response: {text}"))),
                ]),
                None,
            ));
        }
        Ok(d)
    }

    pub(super) fn link(&mut self) -> R<()> {
        let l = self.ask.link.clone();
        let address = self.ask.address.clone();
        let provider = l.provider.as_str();
        if l.request {
            let d = self.call(
                Method::Post,
                "/cli/mail/calendar/link-request",
                Vec::new(),
                Some(obj(vec![
                    ("project", s(&self.ws)),
                    ("address", s(&address)),
                    ("provider", s(provider)),
                ])),
                60,
                line::REQUEST_CALL,
            )?;
            if self.ask.json {
                self.print(&dumps(&d));
            } else {
                let h = at(&d, "hint").filter(|h| py::truthy(h)).cloned();
                self.print(&h.map_or_else(|| "card posted".to_string(), |h| py::str_of(&h)));
            }
            return Ok(());
        }
        let features = if l.write { "calendar" } else { "calendar-read" };
        let gmail = matches!(provider, "google" | "gmail");
        let mut start = vec![
            ("address", s(&address)),
            ("provider", s(if gmail { "gmail" } else { "microsoft" })),
            ("workspace", s(&l.workspace)),
            ("features", s(features)),
        ];
        let mut paste = l.paste || !l.paste_file.is_empty();
        let start_route = "/cli/mail/link/oauth/start";
        let mut d = Value::Null;
        if gmail && !paste {
            let (_, got) = self.call_or_owner(
                Method::Post,
                start_route,
                Some(&obj(start.clone())),
                Vec::new(),
                45,
                line::START,
            )?;
            dict(&got, line::REMOTE, MODULE)?;
            let remote = matches!(at(&got, "error"), Some(e @ Value::Object(_)) if is_str(at(e, "code"), "remote_unsupported"));
            if remote {
                eprint(NO_BROWSER);
                paste = true;
            } else {
                d = self.must(got, line::START_MUST, provider, &address)?;
            }
        }
        if gmail && paste {
            start.push(("clientCapture", Value::Bool(true)));
            start.push((
                "redirectUri",
                s(&format!("http://127.0.0.1:{}/cb", random_port())),
            ));
            start.push(("captureTtlSecs", Value::Int("900".into())));
            let (_, got) = self.call_or_owner(
                Method::Post,
                start_route,
                Some(&obj(start.clone())),
                Vec::new(),
                45,
                line::PASTE_START,
            )?;
            d = self.must(got, line::PASTE_START, provider, &address)?;
            let url = at(&d, "authorizationUrl")
                .cloned()
                .unwrap_or_else(|| s("?"));
            eprint(&format!(
                "1. Open this address in any browser and allow read-only access to {address}'s calendar:\n\n{}\n\n\
                 2. The browser then fails to load a 127.0.0.1 page. That's expected: copy the whole \
                 address from its address bar.\n",
                py::str_of(&url)
            ));
            let pasted = if l.paste_file.is_empty() {
                eprint(PASTE_PROMPT);
                // `sys.stdin.readline()` on the script's own heredoc.
                String::new()
            } else {
                match std::fs::read(&l.paste_file) {
                    Ok(bytes) => match pylib::json::decode_utf8(&bytes) {
                        Ok(t) => strip(&t.replace("\r\n", "\n").replace('\r', "\n")).to_string(),
                        Err(f) => return Err(Tb::at(line::PASTE_READ, MODULE, f.last_line())),
                    },
                    Err(e) => {
                        return Err(self.die(
                            &obj(vec![
                                ("code", s("usage")),
                                (
                                    "hint",
                                    s(&format!("could not read --paste-file: {}", strerror(&e))),
                                ),
                            ]),
                            None,
                        ))
                    }
                }
            };
            let pasted = strip(&pasted).to_string();
            let (code, state, error) = (
                one(&pasted, "code"),
                one(&pasted, "state"),
                one(&pasted, "error"),
            );
            if code.is_empty() && error.is_empty() {
                return Err(self.die(
                    &obj(vec![("code", s("usage")), ("hint", s(NOT_GOOGLE))]),
                    None,
                ));
            }
            let body = obj(vec![
                ("linkId", at(&d, "linkId").cloned().unwrap_or(Value::Null)),
                ("code", s(&code)),
                ("state", s(&state)),
                ("error", s(&error)),
            ]);
            let (status, c) = self.call_or_owner(
                Method::Post,
                "/cli/mail/link/oauth/complete",
                Some(&body),
                Vec::new(),
                60,
                line::COMPLETE,
            )?;
            if matches!(status, 404 | 410) {
                return Err(self.die(
                    &obj(vec![("code", s("expired")), ("hint", s(EXPIRED))]),
                    None,
                ));
            }
            self.must(c, line::COMPLETE_MUST, provider, &address)?;
        } else if !gmail {
            let (_, got) = self.call_or_owner(
                Method::Post,
                start_route,
                Some(&obj(start.clone())),
                Vec::new(),
                45,
                line::MS_START,
            )?;
            d = self.must(got, line::MS_START, provider, &address)?;
            let url = at(&d, "verificationUrl").cloned().unwrap_or_else(|| s("?"));
            let code = at(&d, "userCode").cloned().unwrap_or_else(|| s("?"));
            eprint(&format!(
                "To link {address} read-only: open {} and enter the code {} — sign in as {address}\n",
                py::str_of(&url),
                py::str_of(&code)
            ));
        } else {
            eprint(&format!(
                "A browser window opened — allow read-only access to {address}'s calendar there.\n"
            ));
        }
        eprint("Waiting for approval (Ctrl-C to stop)…\n");
        let link_id = at(&d, "linkId").cloned().unwrap_or(Value::Null);
        let deadline = Instant::now() + DEADLINE;
        let mut fin = None;
        while Instant::now() < deadline {
            let (_, s2) = self.call_or_owner(
                Method::Get,
                "/cli/mail/link/oauth/status",
                None,
                vec![("linkId".to_string(), py::str_of(&link_id))],
                60,
                line::STATUS,
            )?;
            dict(&s2, line::STATUS_ERROR, MODULE)?;
            if let Some(e @ Value::Object(_)) = at(&s2, "error") {
                return Err(self.die(&self.owner_hint(e, provider, &address), None));
            }
            let state = at(&s2, "state");
            if ["connected", "denied", "expired", "error"]
                .iter()
                .any(|x| state.is_some_and(|v| pylib::ops::py_eq(v, &s(x))))
            {
                fin = Some(s2);
                break;
            }
            std::thread::sleep(PAUSE);
        }
        let Some(mut fin) = fin else {
            return Err(self.die(
                &obj(vec![("code", s("timeout")), ("hint", s(TIMED_OUT))]),
                Some(Exit::Usage),
            ));
        };
        if !is_str(at(&fin, "state"), "connected") {
            let mut hint = at(&fin, "hint")
                .filter(|h| py::truthy(h))
                .cloned()
                .unwrap_or_else(|| s(&format!("the link {}", st(at(&fin, "state")))));
            if let Some(url) = at(&fin, "adminConsentUrl").filter(|u| py::truthy(u)) {
                if !py_in(url, &hint, line::FINAL_CONSENT).map_err(raise)? {
                    hint = add(
                        &hint,
                        &format!(" Admin consent: {}", py::str_of(url)),
                        line::FINAL_ADD,
                    )?;
                }
            }
            let code = [at(&fin, "code"), at(&fin, "state")]
                .into_iter()
                .flatten()
                .find(|v| py::truthy(v))
                .cloned()
                .unwrap_or_else(|| s("error"));
            return Err(self.die(
                &obj(vec![("code", code), ("hint", hint)]),
                Some(Exit::Error),
            ));
        }
        let cals: Vec<String> = l
            .cals
            .split(',')
            .map(strip)
            .filter(|x| !x.is_empty() && x.to_lowercase() != "primary")
            .map(str::to_string)
            .collect();
        let mut extra: Vec<(&str, Value)> = Vec::new();
        if !cals.is_empty() {
            let add = Value::Array(cals.iter().map(|c| s(c)).collect());
            let (_, got) = self.call_or_owner(
                Method::Post,
                "/cli/mail/calendar/calendars",
                Some(&obj(vec![("address", s(&address)), ("add", add)])),
                Vec::new(),
                60,
                line::CALS,
            )?;
            extra.push(("calendars", self.must(got, line::CALS, provider, &address)?));
        }
        if l.omit {
            let (_, got) = self.call_or_owner(
                Method::Post,
                "/cli/mail/access/calendar-privacy",
                Some(&obj(vec![
                    ("address", s(&address)),
                    ("project", s(&l.workspace)),
                    ("omitDescription", Value::Bool(true)),
                ])),
                Vec::new(),
                60,
                line::PRIVACY,
            )?;
            extra.push((
                "privacy",
                self.must(got, line::PRIVACY, provider, &address)?,
            ));
        }
        if self.ask.json {
            if let Value::Object(pairs) = &mut fin {
                for (k, v) in extra {
                    match pairs.iter_mut().find(|(key, _)| key.as_str() == Some(k)) {
                        Some(slot) => slot.1 = v,
                        None => pairs.push((Text::from(k), v)),
                    }
                }
            }
            self.print(&dumps(&fin));
        } else {
            self.print(&format!(
                "linked {address} ({}) -> workspace {}",
                if l.write {
                    "read and write"
                } else {
                    "read-only"
                },
                l.workspace
            ));
            let h = at(&fin, "hint").filter(|h| py::truthy(h)).cloned();
            self.print(&h.map_or_else(String::new, |h| py::str_of(&h)));
            if !cals.is_empty() {
                let got = extra
                    .iter()
                    .find(|(k, _)| *k == "calendars")
                    .map(|(_, v)| v.clone())
                    .unwrap_or(Value::Null);
                let added = at(&got, "added").filter(|a| py::truthy(a)).cloned();
                let list =
                    added.unwrap_or_else(|| Value::Array(cals.iter().map(|c| s(c)).collect()));
                let joined = join(&list, ", ", line::CALS_ON, MODULE)?;
                self.print(&format!("calendars on: primary, {joined}"));
            }
            if l.omit {
                self.print("event descriptions are left out for that workspace's agents");
            }
        }
        Ok(())
    }
}

/// `sys.stderr.write(text)`, flushed.
fn eprint(text: &str) {
    let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
}

/// An exception from the shared Python helpers, as this module's.
fn raise(r: crate::families::Raise) -> super::End {
    super::End::Raise(Tb {
        frames: r.frames,
        exception: r.exception,
    })
}

/// `hint += text`: a string grows; a list takes each character.
fn add(hint: &Value, text: &str, line: u32) -> R<Value> {
    match hint {
        Value::Str(_) => Ok(s(&format!("{}{text}", py::str_of(hint)))),
        Value::Array(items) => {
            let mut items = items.clone();
            items.extend(text.chars().map(|c| s(&c.to_string())));
            Ok(Value::Array(items))
        }
        other => Err(Tb::at(
            line,
            MODULE,
            format!(
                "TypeError: unsupported operand type(s) for +=: '{}' and 'str'",
                py::type_name(other)
            ),
        )),
    }
}

/// `e.strerror`: the C library's words, without Rust's ` (os error N)`.
fn strerror(e: &std::io::Error) -> String {
    let text = match e.raw_os_error() {
        Some(code) => std::io::Error::from_raw_os_error(code).to_string(),
        None => e.to_string(),
    };
    match text.rfind(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pasted_address_is_parse_qs_s() {
        let u = "http://127.0.0.1:34567/cb?state=s%2F1&code=4/0A+b&scope=x#frag?code=no";
        assert_eq!(one(u, "code"), "4/0A b");
        assert_eq!(one(u, "state"), "s/1");
        assert_eq!(one(u, "error"), "");
        assert_eq!(one("code=1", "code"), "");
        assert_eq!(one("x?code=&code=2", "code"), "2");
        assert_eq!(one("x?code", "code"), "");
        assert_eq!(one("x?error=access_denied%zz", "error"), "access_denied%zz");
        assert_eq!(one("x?c%6Fde=%C3%A9", "code"), "é");
    }

    /// Every name a line prints is the brand's: an xy build says `xy`
    /// and `XY`, and no line says k2, k5 or K2.
    #[test]
    fn the_lines_name_the_brand() {
        use k2_client::brand::{CLI_NAME, PRODUCT_NAME};
        let lines = [
            NOT_GOOGLE.to_string(),
            EXPIRED.to_string(),
            TIMED_OUT.to_string(),
            NO_BROWSER.to_string(),
            owner_ask("a@x", ""),
        ];
        for l in &lines {
            assert!(
                l.contains(&format!("'{CLI_NAME} cal link")) || l.contains(PRODUCT_NAME),
                "{l:?}"
            );
        }
        assert!(owner_ask("a@x", "").ends_with(&format!(
            "'{CLI_NAME} cal link --request a@x --provider google'"
        )));
        if CLI_NAME != "k2" {
            for l in &lines {
                for name in ["k2 ", "k5 ", "'k2", "K2", "K5"] {
                    assert!(!l.contains(name), "{name:?} in {l:?}");
                }
            }
        }
    }

    #[test]
    fn the_port_is_in_the_shell_s_range() {
        for _ in 0..50 {
            assert!((20000..60000).contains(&random_port()));
        }
    }
}
