//! The confirm flows: `_compute_admin_held_or_approve` (a held change),
//! `_compute_admin_approve` (the owner approves a request in person) and
//! `take-owner` (the node's name typed on the terminal, then the owner's
//! confirm). The confirm token goes from the vault socket's answer into
//! the next request's body, never into argv or the environment.

use super::print;
use crate::confirm::{confirm_token, Confirm};
use crate::exit::Exit;
use crate::families::compute::lines::py_line;
use crate::families::compute::pyrt::{self as p, s};
use crate::families::compute::render::{self, Cx};
use crate::families::compute::writes::{body_of, dumps, in_session, object, str_value, Cp};
use crate::families::{raised, Raise, Shown};
use crate::out;
use crate::out::json::Value;
use crate::prompt;
use crate::transport::encode::quote;
use crate::transport::permission;
use k2_client::{brand::CLI_NAME, product_name};
use std::path::Path;

const SRC: &str = super::SRC;

macro_rules! line {
    ($after:expr, $marker:literal) => {
        const { py_line(SRC, $after, $marker) }
    };
}

/// Text on stderr as it is.
pub fn stderr_text(text: &str) {
    let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
}

/// `grep -q '"held": *true'` on the answer.
pub fn held(body: &[u8]) -> bool {
    let key = b"\"held\":";
    let mut i = 0;
    while i + key.len() <= body.len() {
        if &body[i..i + key.len()] == key {
            let mut j = i + key.len();
            while body.get(j) == Some(&b' ') {
                j += 1;
            }
            if body[j..].starts_with(b"true") {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// A Python run whose output goes to stderr (`python3 -c '…' >&2`): what
/// it printed and, if it raised, the traceback. Python's stdout is a
/// file then, written when Python exits, after the traceback; on a
/// terminal each line goes out as it is printed.
pub fn to_stderr(cx: Cx, r: Result<(), Raise>) -> Result<(), Exit> {
    match r {
        Ok(()) => {
            stderr_text(&cx.out);
            Ok(())
        }
        Err(e) => {
            if prompt::stderr_is_tty() {
                stderr_text(&cx.out);
                stderr_text(&e.text());
            } else {
                stderr_text(&e.text());
                stderr_text(&cx.out);
            }
            Err(Exit::Error)
        }
    }
}

/// `x="$(printf '%s' "$_CP_BODY" | python3 -c '…print(<one value>)')"`
/// under `set -e`: the value as `print` writes it, trailing newlines
/// gone; a raise is the traceback and exit 1.
pub fn captured(r: Result<Value, Raise>) -> Result<String, Exit> {
    match r {
        Ok(v) => Ok(p::str_of(&v).trim_end_matches('\n').to_string()),
        Err(e) => {
            stderr_text(&e.text());
            Err(Exit::Error)
        }
    }
}

pub fn confirm<'a>(cp: &'a mut Cp<'_>, sock: &'a Path) -> Confirm<'a> {
    let passport = cp.client.passport().is_some();
    Confirm {
        client: &mut *cp.client,
        sock,
        closing: &mut cp.closing,
        passport,
    }
}

/// `_compute_admin_held_or_approve`: the request id and summary on
/// stderr; the owner at a terminal outside a session approves it now
/// (exit 0); anyone else gets where to approve it and exit 2.
pub fn held_or_approve(cp: &mut Cp<'_>, sock: &Path, body: &[u8]) -> Exit {
    let cx = Cx::new(0);
    let r = render::load(&cx, body, 1).and_then(|v| {
        let r = cx.at(1, p::idx(&v, "request"))?;
        cx.at(1, p::idx(&r, "id"))
    });
    let id = match captured(r) {
        Ok(id) => id,
        Err(e) => return e,
    };
    const SHOW: &str = "[\"request\"][\"id\"])')\"";
    let mut cx = Cx::new(0);
    let l = line!(SHOW, "v=json.load(sys.stdin); r=");
    let r = render::load(&cx, body, l)
        .and_then(|v| cx.at(l, p::idx(&v, "request")))
        .and_then(|r| {
            let l = line!(SHOW, "print(\"held for the owner");
            let id = cx.at(l, p::idx(&r, "id"))?;
            let summary = cx.at(l, p::idx(&r, "summary"))?;
            print(&mut cx, l, "held for the owner: %s\n  %s", &[id, summary])
        });
    if let Err(e) = to_stderr(cx, r) {
        return e;
    }
    let no_approve =
        k2_client::env_names::first_nonempty_os_in(k2_client::env_names::NO_APPROVE, |n| {
            std::env::var_os(n)
        })
        .is_some_and(|(_, v)| v == "1");
    if !in_session(&cp.client.auth) && prompt::stdin_is_tty() && !no_approve {
        return approve(cp, sock, &id);
    }
    out::stderr_line(&format!(
        concat!(
            "  the owner approves it in Settings → Cores → Compute → Machine settings → Waiting for you (or: {} compute node admin approve {}, outside ",
            product_name!(),
            ")"
        ),
        CLI_NAME, id
    ));
    if permission::note(body).is_some_and(|n| !n.lines().is_empty()) {
        out::stderr_line("  to ask the owner in chat now, run the Ask with: line below: it puts this same request on a card.");
        out::stderr_line("  Allow there sends it to the node, as Settings would (don't run your command again); your [permission:] turn says how it went");
    }
    Exit::Usage
}

/// `_compute_admin_approve <id>`.
pub fn approve(cp: &mut Cp<'_>, sock: &Path, want: &str) -> Exit {
    if in_session(&cp.client.auth) {
        out::stderr_line(concat!(
            "{\"error\":\"owner_only\",\"hint\":\"approve in Settings → Cores → Compute → Machine settings → Waiting for you, or from a terminal outside ",
            product_name!(),
            ". An agent can never approve.\"}"
        ));
        return Exit::Refused;
    }
    let list = match cp.call(
        false,
        "/cli/compute/admin/requests",
        "state=all&limit=500",
        true,
    ) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let mut cx = Cx::new(0);
    let found = find(&mut cx, &list, want);
    let found = match found {
        Ok(true) => cx.out.trim_end_matches('\n').to_string(),
        Ok(false) | Err(_) => {
            if let Err(e) = &found {
                stderr_text(&e.text());
            }
            out::stderr_line(&format!(
                "{{\"error\":\"request_not_found\",\"hint\":\"no single request matches '{want}' ({CLI_NAME} compute node admin list)\"}}"
            ));
            return Exit::Refused;
        }
    };
    let lines: Vec<&str> = found.split('\n').collect();
    let nth = |i: usize| lines.get(i).copied().unwrap_or("");
    let (full, state, label) = (nth(0), nth(1), nth(2));
    if state != "pending" {
        out::stderr_line(&format!(
            "{{\"error\":\"not_pending\",\"hint\":\"{full} is {state}\"}}"
        ));
        return Exit::Refused;
    }
    for l in lines.iter().skip(4) {
        out::stderr_line(l);
    }
    let Some(tok) = confirm_token(&mut confirm(cp, sock), "node-admin", full, label) else {
        return Exit::Refused;
    };
    let body = body_of(&[("confirmToken", &tok), ("request", full)]);
    drop(tok);
    let answer = match cp.call(true, "/cli/compute/admin/approve", &body, true) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let mut cx = Cx::new(0);
    const AFTER: &str = "/cli/compute/admin/approve";
    let l = line!(AFTER, "v=json.load(sys.stdin); r=");
    let r = render::load(&cx, &answer, l)
        .and_then(|v| Ok((cx.at(l, p::idx(&v, "request"))?, v)))
        .and_then(|(r, v)| {
            let l = line!(AFTER, "print(\"approved %s");
            let args = [
                cx.at(l, p::idx(&r, "id"))?,
                cx.at(l, p::idx(&r, "summary"))?,
                cx.at(l, p::get_or(&v, "note", s("")))?,
            ];
            print(&mut cx, l, "approved %s: %s\n  %s", &args)
        });
    match r {
        Ok(()) => Shown::out(std::mem::take(&mut cx.out)).print(),
        Err(e) => raised(std::mem::take(&mut cx.out), String::new(), &e).print(),
    }
}

/// The approve flow's finder: `Ok(true)` with its lines printed, `Ok(false)`
/// for `sys.exit(1)` (not one match), `Err` for a raise.
fn find(cx: &mut Cx, body: &[u8], want: &str) -> Result<bool, Raise> {
    const AFTER: &str = "\"state=all&limit=500\" tcp";
    let l = line!(AFTER, "rows = [r for r in");
    let v = render::load(cx, body, l)?;
    let all = cx.at(l, p::idx(&v, "requests"))?;
    let mut rows = vec![];
    for r in cx.at(l, p::iter(&all))? {
        let id = cx.at(l, p::idx(&r, "id"))?;
        if p::eq_s(&id, want) || cx.at(l, p::startswith(&id, want))? {
            rows.push(r);
        }
    }
    if rows.len() != 1 {
        return Ok(false);
    }
    let r = rows.remove(0);
    let l = line!(AFTER, "print(r[\"id\"]); print(r[\"state\"])");
    let id = cx.at(l, p::idx(&r, "id"))?;
    cx.print(&p::str_of(&id));
    let state = cx.at(l, p::idx(&r, "state"))?;
    cx.print(&p::str_of(&state));
    let label = cx.at(
        l,
        p::idx(&r, "summary").and_then(|x| p::slice(&x, None, Some(64))),
    )?;
    cx.print(&p::str_of(&label));
    let l = line!(AFTER, "print(\"---\"); print(r[\"summary\"])");
    cx.print("---");
    let summary = cx.at(l, p::idx(&r, "summary"))?;
    cx.print(&p::str_of(&summary));
    let l = line!(AFTER, "if r.get(\"reason\"):");
    if p::truthy(&cx.at(l, p::get(&r, "reason"))?) {
        let x = cx.at(l, p::idx(&r, "reason"))?;
        let t = cx.at(l, p::add(&s("reason: "), &x))?;
        cx.print(&p::str_of(&t));
    }
    let l = line!(AFTER, "for s in (r.get(\"body\")");
    let body = p::or(cx.at(l, p::get(&r, "body"))?, Value::Object(vec![]));
    let steps = cx.at(l, p::get(&body, "steps"))?;
    let steps = if p::truthy(&steps) {
        steps
    } else {
        let b = p::or(cx.at(l, p::get(&r, "body"))?, Value::Object(vec![]));
        Value::Array(vec![b])
    };
    for st in cx.at(l, p::iter(&steps))? {
        let l = line!(AFTER, "if s.get(\"verb\") == \"script\":");
        if !p::eq_s(&cx.at(l, p::get(&st, "verb"))?, "script") {
            continue;
        }
        let l = line!(AFTER, "print(\"--- script (%s) ---\"");
        let args = p::or(cx.at(l, p::get(&st, "args"))?, Value::Object(vec![]));
        let interp = cx.at(l, p::get_or(&args, "interpreter", s("/bin/sh")))?;
        print(cx, l, "--- script (%s) ---", &[interp])?;
        let l = line!(AFTER, "print((s.get(\"args\") or {}).get(\"text\"");
        let args = p::or(cx.at(l, p::get(&st, "args"))?, Value::Object(vec![]));
        let text = cx.at(l, p::get_or(&args, "text", s("")))?;
        cx.print(&p::str_of(&text));
    }
    Ok(true)
}

/// `[ -r /dev/tty ]` as bash tests it: the file's mode, not whether this
/// process has a terminal to open.
pub fn dev_tty_r_ok() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: a NUL-terminated path; access only reads it.
        unsafe { libc::access(c"/dev/tty".as_ptr(), libc::R_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// `take-owner <node>`.
pub fn take_owner(cp: &mut Cp<'_>, sock: &Path, node: &str) -> Exit {
    let preview = dumps(&object(vec![
        ("node", str_value(node)),
        ("preview", Value::Bool(true)),
    ]));
    let answer = match cp.call(true, "/cli/compute/nodes/take-owner", &preview, true) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let one = |key: &str| {
        let cx = Cx::new(0);
        captured(render::load(&cx, &answer, 1).and_then(|v| cx.at(1, p::idx(&v, key))))
    };
    let tname = match one("node") {
        Ok(t) => t,
        Err(e) => return e,
    };
    let tfp = match one("fingerprint") {
        Ok(t) => t,
        Err(e) => return e,
    };
    let mut cx = Cx::new(0);
    let r = take_over_text(&mut cx, &answer);
    if let Err(e) = to_stderr(cx, r) {
        return e;
    }
    if !dev_tty_r_ok() {
        out::stderr_line("{\"error\":\"needs_a_person\",\"hint\":\"take-owner asks you to type the node name: run it in a terminal\"}");
        return Exit::Refused;
    }
    stderr_text(&format!("Type the node name ({tname}) to take it over: "));
    let typed = prompt::read_dev_tty_line()
        .map(|t| t.expose().to_vec())
        .unwrap_or_default();
    if typed != tname.as_bytes() {
        out::stderr_line(
            "{\"error\":\"not_confirmed\",\"hint\":\"the name did not match; nothing changed\"}",
        );
        return Exit::Refused;
    }
    let info = match cp.call(
        false,
        "/cli/compute/nodes/info",
        &format!("node={}", quote(&tname)),
        false,
    ) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let cx = Cx::new(0);
    let tid = match captured(render::load(&cx, &info, 1).and_then(|v| cx.at(1, p::idx(&v, "id")))) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let secret_id = format!("take-owner:{tid}");
    let label = format!("take over {tname} ({tfp})");
    let Some(tok) = confirm_token(&mut confirm(cp, sock), "node-admin", &secret_id, &label) else {
        return Exit::Refused;
    };
    let body = body_of(&[("confirmToken", &tok), ("node", &tid)]);
    drop(tok);
    let answer = match cp.call(true, "/cli/compute/nodes/take-owner", &body, true) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let mut cx = Cx::new(0);
    let r = render::load(&cx, &answer, 2).and_then(|v| {
        let node = cx.at(2, p::idx(&v, "node"))?;
        let from = p::or(cx.at(2, p::get(&v, "from"))?, s("its owner"));
        print(
            &mut cx,
            2,
            "%s is this server's now (taken over from %s): its admin, updates, servers and approvals are here.",
            &[node, from],
        )
    });
    match r {
        Ok(()) => Shown::out(std::mem::take(&mut cx.out)).print(),
        Err(e) => raised(std::mem::take(&mut cx.out), String::new(), &e).print(),
    }
}

/// The preview's text (to stderr).
fn take_over_text(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const AFTER: &str = "[\"fingerprint\"])')\"";
    let v = render::load(cx, body, line!(AFTER, "v=json.load(sys.stdin)"))?;
    let l = line!(AFTER, "print(\"Take over %s?\"");
    let x = cx.at(l, p::idx(&v, "node"))?;
    print(cx, l, "Take over %s?", &[x])?;
    let l = line!(AFTER, "print(\"  node key (full)");
    let x = cx.at(l, p::idx(&v, "fingerprint"))?;
    print(cx, l, "  node key (full): %s", &[x])?;
    let l = line!(AFTER, "print(\"  owner until now");
    let x = p::or(cx.at(l, p::get(&v, "from"))?, s("another server"));
    print(cx, l, "  owner until now: %s", &[x])?;
    cx.print("  The node only SAYS this server owns it. Go on only if you changed its owner yourself and that key is the node you know.");
    cx.print("  This server becomes its admin (root admin on that machine: updates, policy, servers, approvals).");
    cx.print("  Every compute:<workspace>/* secret grant starts sending its secrets to it:");
    let l = line!(AFTER, "if not v.get(\"grantsKnown\"):");
    if !p::truthy(&cx.at(l, p::get(&v, "grantsKnown"))?) {
        let l = line!(AFTER, "print(\"    \" + v[\"secretGrantsText\"])");
        let x = cx.at(l, p::idx(&v, "secretGrantsText"))?;
        let t = cx.at(l, p::add(&s("    "), &x))?;
        cx.print(&p::str_of(&t));
        return Ok(());
    }
    let l = line!(AFTER, "elif not v.get(\"secretGrants\"):");
    if !p::truthy(&cx.at(l, p::get(&v, "secretGrants"))?) {
        cx.print("    none today");
        return Ok(());
    }
    let l = line!(AFTER, "for g in v[\"secretGrants\"]:");
    let grants = cx.at(l, p::idx(&v, "secretGrants"))?;
    for g in cx.at(l, p::iter(&grants))? {
        let l = line!(AFTER, "print(\"    %s: %s\"");
        let w = cx.at(l, p::idx(&g, "workspace"))?;
        let sec = cx.at(l, p::idx(&g, "secret"))?;
        print(cx, l, "    %s: %s", &[w, sec])?;
    }
    Ok(())
}
