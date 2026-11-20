//! `k5 compute events [--follow] [--mine] [--tag T] [--since CURSOR]
//! [--limit N] [--max-wait D] [--json]`: your jobs' events, as the
//! shell's `events` arm (`cli/src/compute-command.sh`) reads them: one
//! `GET /cli/compute/jobs?events=1…` (`last=N` the first time, `after=` a
//! cursor from then on; `--follow` asks the daemon to wait up to 20 s,
//! less near `--max-wait`), the events (one line each, or compact JSON),
//! and the next cursor on stderr at the end. `--audit` is the owner's
//! audit log ([`super::render`]'s `events`).

use super::lines::{py_line, COMMAND};
use super::pyrt::{self as p, s};
use super::reads::{call, ws};
use super::render::{load, Cx};
use crate::exit::Exit;
use crate::families::{raised, Raise, Shown};
use crate::out;
use crate::prelude::Closing;
use crate::transport::encode::quote;
use crate::transport::Client;
use k2_client::cli_name;

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Events {
    pub follow: bool,
    pub mine: bool,
    pub tag: String,
    pub since: String,
    pub limit: String,
    /// `--max-wait` in seconds (`_compute_secs`).
    pub max_wait: Option<i64>,
    pub json: bool,
}

/// `_compute_secs`: plain seconds, or `<n>h`, `<n>m` and `<n>s` parts.
/// `None`: a duration it refuses (inside `$(( ))`, where the shell's own
/// error follows: the shell's form).
pub fn secs(v: &str) -> Option<i64> {
    if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
        return v.parse().ok();
    }
    let (mut total, mut n, mut digits) = (0i64, 0i64, false);
    for c in v.chars() {
        match c {
            '0'..='9' => {
                n = n.checked_mul(10)?.checked_add(i64::from(c as u8 - b'0'))?;
                digits = true;
            }
            'h' | 'm' | 's' if digits => {
                let unit = match c {
                    'h' => 3600,
                    'm' => 60,
                    _ => 1,
                };
                total = total.checked_add(n.checked_mul(unit)?)?;
                n = 0;
                digits = false;
            }
            _ => return None,
        }
    }
    (!v.is_empty() && !digits).then_some(total)
}

const fn line(marker: &str) -> u32 {
    py_line(
        COMMAND,
        "eout=\"$(printf '%s' \"$_CP_BODY\" | K2_CP_JSON=",
        marker,
    )
}

fn now() -> i64 {
    p::now()
}

pub fn run(client: &mut Client, closing: &mut Closing, a: &Events, project: &str) -> Exit {
    let mut eq = "events=1".to_string();
    if a.mine {
        eq.push_str("&mine=1");
    }
    if !a.tag.is_empty() {
        eq.push_str(&format!("&tag={}", quote(&a.tag)));
    }
    let w = ws(client, project);
    if !w.is_empty() {
        eq.push_str(&format!("&workspace={}", quote(&w)));
    }
    let deadline = a.max_wait.map(|m| now() + m);
    let mut cursor = a.since.clone();
    let mut first = true;
    loop {
        let mut q = eq.clone();
        if !cursor.is_empty() {
            q.push_str(&format!("&after={}", quote(&cursor)));
        } else if first && !a.follow {
            let limit = if a.limit.is_empty() { "20" } else { &a.limit };
            q.push_str(&format!("&last={}", quote(limit)));
        }
        if a.follow {
            let mut wait = 20;
            if let Some(d) = deadline {
                wait = wait.min((d - now()).max(0));
            }
            q.push_str(&format!("&wait={wait}"));
        }
        let body = match call(client, closing, "/cli/compute/jobs", &q, false, false) {
            Ok(b) => b,
            Err(e) => return e,
        };
        let shown = printer(&body, a.json);
        if shown.exit != Exit::Ok {
            // `eout="$(…)"`: its stdout is held; the traceback shows.
            return Shown {
                stdout: String::new(),
                ..shown
            }
            .print();
        }
        // `$(…)` drops the trailing newlines; the cursor is after the
        // last one left, and what comes before it is printed.
        let eout = shown.stdout.trim_end_matches('\n');
        match eout.rfind('\n') {
            Some(i) => {
                cursor = eout[i + 1..].to_string();
                if let Err(e) = out::stdout(&format!("{}\n", &eout[..i])) {
                    return out::after_stdout_error(&e);
                }
            }
            None => cursor = eout.to_string(),
        }
        first = false;
        if !a.follow {
            break;
        }
        if deadline.is_some_and(|d| now() >= d) {
            out::stderr_line(&format!(
                concat!(
                    "[",
                    cli_name!(),
                    " compute] --max-wait passed; continue with: ",
                    cli_name!(),
                    " compute events --follow --since {}"
                ),
                cursor
            ));
            return Exit::Ok;
        }
    }
    out::stderr_line(&format!(
        concat!(
            "[",
            cli_name!(),
            " compute] next: ",
            cli_name!(),
            " compute events --follow --since {}"
        ),
        cursor
    ));
    Exit::Ok
}

/// The events' Python: a line per event (or compact JSON), then the
/// cursor.
pub fn printer(body: &[u8], json: bool) -> Shown {
    let mut cx = Cx::new(now());
    match print(&mut cx, body, json) {
        Ok(()) => Shown::out(cx.out),
        Err(r) => raised(std::mem::take(&mut cx.out), String::new(), &r),
    }
}

fn print(cx: &mut Cx, body: &[u8], json: bool) -> Result<(), Raise> {
    let v = load(cx, body, const { line("v = json.load(sys.stdin)") })?;
    let l = const { line("for e in v.get(\"events\") or []:") };
    let es = cx.at(l, p::get(&v, "events"))?;
    for e in cx.at(l, p::iter_or_empty(es))? {
        if json {
            let l = const { line("print(json.dumps(e, separators=(\",\", \":\")))") };
            let t = cx.at(l, p::dumps(&e, true, None))?;
            cx.print(&t);
            continue;
        }
        let g = |cx: &Cx, l: u32, k: &str| cx.at(l, p::get(&e, k));
        let mut tail = String::new();
        let l = const { line("if e.get(\"event\") == \"finished\":") };
        if p::eq_s(&g(cx, l, "event")?, "finished") {
            let l = const { line("tail = \": %s\" % e.get(\"state\")") };
            tail = cx.at(l, p::pct(": %s", &[g(cx, l, "state")?]))?;
            let l = const { line("if e.get(\"exit\") is not None:") };
            if !p::is_none(&g(cx, l, "exit")?) {
                let l = const { line("tail += \" exit %s\" % e[\"exit\"]") };
                let x = cx.at(l, p::idx(&e, "exit"))?;
                tail += &cx.at(l, p::pct(" exit %s", &[x]))?;
            }
            let l = const { line("if e.get(\"reason\"):") };
            if p::truthy(&g(cx, l, "reason")?) {
                let l = const { line("tail += \" (%s)\" % e[\"reason\"]") };
                let x = cx.at(l, p::idx(&e, "reason"))?;
                tail += &cx.at(l, p::pct(" (%s)", &[x]))?;
            }
        }
        let l = const { line("if e.get(\"detail\"):") };
        let at = |cx: &mut Cx, l: u32| -> Result<String, Raise> {
            let t = p::or(g(cx, l, "at")?, p::int(0));
            let n = cx.now();
            cx.at(l, p::strftime("%H:%M:%S", &t, n))
        };
        if p::truthy(&g(cx, l, "detail")?) {
            let l = const { line("print(\"%s  %s  %s\" % (time.strftime") };
            let when = at(cx, l)?;
            let short = g(cx, l, "short")?;
            let d = cx.at(l, p::idx(&e, "detail"))?;
            let t = cx.at(l, p::pct("%s  %s  %s", &[s(&when), short, d]))?;
            cx.print(&t);
            continue;
        }
        let l = const { line("print(\"%s  %s  %-9s on %s%s\"") };
        let when = at(cx, l)?;
        let short = g(cx, l, "short")?;
        let event = g(cx, l, "event")?;
        let node = p::or(g(cx, l, "node")?, s("?"));
        let t = cx.at(
            l,
            p::pct(
                "%s  %s  %-9s on %s%s",
                &[s(&when), short, event, node, s(&tail)],
            ),
        )?;
        cx.print(&t);
    }
    let l = const { line("print(v.get(\"cursor\"))") };
    let c = cx.at(l, p::get(&v, "cursor"))?;
    cx.print(&p::str_of(&c));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_as_compute_secs() {
        for (v, want) in [
            ("300", Some(300)),
            ("007", Some(7)),
            ("2h", Some(7200)),
            ("1h30m", Some(5400)),
            ("90s", Some(90)),
            ("1h1", None),
            ("h", None),
            ("", None),
            ("1d", None),
            ("1.5h", None),
        ] {
            assert_eq!(secs(v), want, "{v}");
        }
    }
}
