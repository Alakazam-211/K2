//! `k5 setup agent-reports`: send-feedback's settings, as `cmd_setup`
//! hands them to `_sf_settings_run setup` (`cli/src/setup.sh`,
//! `cli/src/send-feedback.sh`): whether agents may report problems to the
//! product's makers, and which parts of the details they may attach.
//!
//! 1. The words after `agent-reports` (with `--json` first when it came
//!    before the verb), read as `_sf_settings_run setup` reads them
//!    ([`super::super::send_feedback::parse_settings_as`]): `--agents`,
//!    `--attach` and `--json`; `--enabled`, `--agent-diagnostics` and any
//!    other word are [`USAGE`], exit 2, nothing sent.
//! 2. The calls send-feedback's settings make
//!    ([`super::super::send_feedback::settings_calls`]), each one
//!    `_sf_settings_call setup` ([`call`]): `_setup_send` over TCP with no
//!    `?format=text`, its answer held in `raw=$(…)`.
//! 3. `--json` prints the last answer and a newline; else
//!    `_sf_settings_render` reads it.
//! 4. The EXIT trap's lines ([`crate::prelude::Closing`]), from a refusal
//!    `_permission_note` was handed.

use super::super::send_feedback::{self as sf, SETTINGS};
use super::{cut_200, error_object, held, jfield, utf8_locale};
use crate::daemon::Settings;
use crate::exit::Exit;
use crate::families::curl_port;
use crate::out;
use crate::passport;
use crate::prelude::Closing;
use crate::transport::{permission, Channel, Client, Response};
use k2_client::{cli_name, product_name};
use std::ffi::OsString;

/// `_sf_settings_run setup`'s usage line.
pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " setup agent-reports [--agents off|cli|all] [--attach <parts>|none|all] [--json]"
);

/// `_sf_settings_call setup` with no answer.
pub const UNREACHABLE: &str = concat!(
    "Error: could not reach the ",
    product_name!(),
    " daemon (",
    cli_name!(),
    " setup agent-reports)"
);

/// `setup agent-reports <words>`, past the gates: `words` are what
/// `cmd_setup` hands `_sf_settings_run setup`.
pub fn run(args: &[OsString], words: &[String], mut settings: Settings) -> Exit {
    let w: Vec<&str> = words.iter().map(String::as_str).collect();
    let a = match sf::parse_settings_as(&w, USAGE, false) {
        Ok(a) => a,
        Err(line) => {
            out::stderr_line(&line);
            return Exit::Usage;
        }
    };
    // A port curl won't take: curl sends nothing and writes no status.
    let Some(port) = settings.port_text.as_deref().and_then(curl_port) else {
        out::stderr_line(UNREACHABLE);
        return Exit::Error;
    };
    settings.port = Some(port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let body = sf::settings_calls(&a, &mut |post, payload| {
        call(&mut client, &mut closing, post, payload)
    });
    let exit = match body {
        Ok(b) if a.json => {
            let mut line = b;
            line.push(b'\n');
            write_out(&line)
        }
        Ok(b) => write_out(&sf::render(&b)),
        Err(e) => e,
    };
    closing.flush(&mut client, exit);
    exit
}

fn write_out(bytes: &[u8]) -> Exit {
    match out::write_flushed(&mut std::io::stdout().lock(), bytes) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// `_sf_settings_call setup <method> [body]`: the answer's body
/// (`_CP_BODY`) on a 200. No answer is [`UNREACHABLE`], exit 1; any other
/// status goes to `_permission_note`, then one JSON line on stderr, the
/// refusal's code and hint (or the error and message; with neither,
/// `error` and the body's first 200 characters), exit 2 for 400 and 413,
/// else 3.
pub fn call(
    client: &mut Client,
    closing: &mut Closing,
    post: bool,
    payload: &str,
) -> Result<Vec<u8>, Exit> {
    let result = client.setup_send(post, SETTINGS, false, payload.as_bytes());
    let raw = held(&result, client.refused_before_answer());
    let (body, code) = match raw.iter().rposition(|b| *b == b'\n') {
        Some(i) => (&raw[..i], &raw[i + 1..]),
        None => (&raw[..], &raw[..]),
    };
    if raw.is_empty() || code == raw.as_slice() || code == b"000" {
        out::stderr_line(UNREACHABLE);
        return Err(Exit::Error);
    }
    if code == b"200" {
        return Ok(body.to_vec());
    }
    let answer = Response {
        status: result.as_ref().map_or(0, |r| r.status),
        body: body.to_vec(),
        channel: Channel::Tcp,
        permission: permission::note(body),
        server_version: None,
    };
    closing.answer(&answer, client.passport().is_some());
    let (err, hint) = match error_object(body) {
        Some(rest) => (jfield("code", rest), jfield("hint", rest)),
        None => (jfield("error", body), jfield("message", body)),
    };
    let (err, hint) = match err.filter(|e| !e.is_empty()) {
        Some(e) => (e, hint.unwrap_or_default()),
        None => (b"error".to_vec(), cut_200(body, utf8_locale).to_vec()),
    };
    let mut line = b"{\"error\": ".to_vec();
    line.extend(jstr(&err));
    line.extend_from_slice(b", \"hint\": ");
    line.extend(jstr(&hint));
    line.extend_from_slice(b"}\n");
    let _ = out::write_flushed(&mut std::io::stderr().lock(), &line);
    Err(match code {
        b"400" | b"413" => Exit::Usage,
        _ => Exit::Refused,
    })
}

/// `_setup_jstr` on bytes, as the C locale reads them: `\` and `"`
/// escaped, newline, tab and carriage return as `\n`, `\t`, `\r`, DEL as
/// it is, every other control byte `\u00xx`, all else as it is.
pub fn jstr(value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len() + 2);
    out.push(b'"');
    for &b in value {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'"' => out.extend_from_slice(b"\\\""),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0..=0x1f => out.extend_from_slice(format!("\\u{b:04x}").as_bytes()),
            _ => out.push(b),
        }
    }
    out.push(b'"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jstr_is_setup_jstr_on_bytes() {
        assert_eq!(jstr(b"plain"), b"\"plain\"");
        assert_eq!(jstr(b"a\"b\\c"), b"\"a\\\"b\\\\c\"");
        assert_eq!(jstr(b"\n\t\r\x01\x7f\xff"), b"\"\\n\\t\\r\\u0001\x7f\xff\"");
        // The same as the string form for text.
        for s in [
            "K \"Five\" \\ 5",
            "runs\tand\nbooks\r",
            "a\u{1}b\u{1f}c\u{7f}d é",
        ] {
            assert_eq!(
                jstr(s.as_bytes()),
                super::super::jstr(s).into_bytes(),
                "{s:?}"
            );
        }
    }

    #[test]
    fn the_usage_line_is_the_shell_s() {
        let src = include_str!("../../../../../cli/src/send-feedback.sh");
        assert!(src.contains(
            "usage=\"usage: k2 setup agent-reports [--agents off|cli|all] [--attach <parts>|none|all] [--json]\""
        ));
        assert!(src.contains(
            "echo \"Error: could not reach the K2 daemon (k2 setup agent-reports)\" >&2"
        ));
    }
}
