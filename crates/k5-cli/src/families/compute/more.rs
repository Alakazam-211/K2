//! More `compute` verbs, as the shell CLI's `cmd_compute` answers them
//! (`cli/src/compute-command.sh` and the files it calls):
//!
//! - `retry <job>` ([`retry`]);
//! - [`trust`]: `roster`, `seats`, `trust` and `owners` (the node owners'
//!   trust verbs of `cmd_compute_trust`; `lanes` is not here).
//!
//! What runs is what [`super::admin`] runs, in the shell's order: the
//! settings, a re-minted passport, the two gates, the `id` tool's lines,
//! the verb's words (usage: exit 2; a `shift 2` with one word left: exit
//! 1, nothing printed), then the verb's requests and readers, then the
//! EXIT trap's lines. What the binary hands to the shell, before it prints
//! or sends anything, is what [`super::admin::run`] hands it.
//!
//! This driver is `admin`'s; the compute modules can share one once the
//! compute lanes have landed.

use super::writes::{self, body_of, Cp, Words};
use crate::daemon;
use crate::exit::Exit;
use crate::families::{curl_port, raised, Shown};
use crate::out;
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use std::ffi::OsString;
use std::path::Path;

pub mod trust;

#[cfg(test)]
mod tests;

/// The second words this module answers.
pub const ANSWERS: &[&str] = &["retry", "roster", "seats", "trust", "owners"];

/// A verb this module answers, with its words read.
enum Ask {
    Retry(String),
    Trust(trust::Ask),
}

/// `k5 compute <verb> ...`: `args` is the whole command line after
/// `argv[0]`. `None`: another module or the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = crate::families::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    if !ANSWERS.contains(words.first()?) {
        return None;
    }
    let max_time = writes::post_max_time(std::env::var_os("K2_CP_MAX_TIME"))?;
    if writes::shell_settings(std::env::vars_os()) {
        return None;
    }
    if !python::family_native(python::here()) {
        return None;
    }
    let mut settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) =
        prelude::id_tool(&settings, hook_sock.as_deref(), would_refuse, writes::TOOL)
    {
        return Some(stop);
    }
    let rest = &words[1..];
    let ask = match words[0] {
        "retry" => match rest.first() {
            Some(job) if !job.is_empty() => Ask::Retry(job.to_string()),
            _ => {
                out::stderr_line(RETRY_USAGE);
                return Some(Exit::Usage);
            }
        },
        noun => match ready(trust::parse(noun, rest)) {
            Ok(a) => Ask::Trust(a),
            Err(exit) => return Some(exit),
        },
    };
    let vault_sock = crate::confirm::sock_path(&settings.home);
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    client.compute_post_max_time = max_time;
    let as_json = words.iter().skip(1).any(|w| *w == "--json");
    let mut cp = Cp {
        client: &mut client,
        closing: Closing::default(),
        project: settings.project.clone(),
        as_json,
    };
    let exit = send(&mut cp, &ask, &vault_sock);
    let closing = std::mem::take(&mut cp.closing);
    closing.flush(&mut client, exit);
    Some(exit)
}

fn send(cp: &mut Cp<'_>, ask: &Ask, sock: &Path) -> Exit {
    match ask {
        Ask::Retry(job) => retry(cp, job),
        Ask::Trust(a) => trust::send(cp, a, sock),
    }
}

/// The ask, or what stopping prints and the exit.
fn ready<T>(w: Words<T>) -> Result<T, Exit> {
    match w {
        Words::Ask(a) => Ok(a),
        Words::Usage(line) => {
            out::stderr_line(&line);
            Err(Exit::Usage)
        }
        Words::ShiftFailed => Err(Exit::Error),
    }
}

pub const RETRY_USAGE: &str = concat!("usage: ", k2_client::cli_name!(), " compute retry <job>");

/// `retry <job>`: `/cli/compute/jobs/retry`, then its one-line reader.
fn retry(cp: &mut Cp<'_>, job: &str) -> Exit {
    let body = body_of(&[("job", job)]);
    match cp.call(true, "/cli/compute/jobs/retry", &body, false) {
        Ok(answer) => retry_reader(&answer).print(),
        Err(e) => e,
    }
}

/// The reader after `/cli/compute/jobs/retry` (`cli/src/compute-command.sh`).
pub fn retry_reader(body: &[u8]) -> Shown {
    use super::lines::{py_line, COMMAND};
    use super::pyrt::{self as p, s};
    use super::render::{self, Cx};
    const AFTER: &str = "/cli/compute/jobs/retry";
    let mut cx = Cx::new(0);
    let r = (|| {
        let l = const { py_line(COMMAND, AFTER, "v = json.load(sys.stdin)") };
        let v = render::load(&cx, body, l)?;
        let l = const { py_line(COMMAND, AFTER, "if \"job\" in v:") };
        if cx.at(l, p::has(&v, "job"))? {
            let job = cx.at(l, p::idx(&v, "job"))?;
            let attempt = cx.at(l, p::idx(&job, "attempt"))?;
            let job = cx.at(l, p::idx(&v, "job"))?;
            let short = cx.at(l, p::idx(&job, "short"))?;
            let fmt = concat!(
                "retrying as attempt %s: ",
                k2_client::cli_name!(),
                " compute attach %s"
            );
            let t = cx.at(l, p::pct(fmt, &[attempt, short]))?;
            cx.print(&t);
        } else {
            let l = const { py_line(COMMAND, AFTER, "else: print(v.get(") };
            let state = cx.at(l, p::get_or(&v, "state", s("ok")))?;
            cx.print(&p::str_of(&state));
        }
        Ok(())
    })();
    match r {
        Ok(()) => Shown::out(std::mem::take(&mut cx.out)),
        Err(e) => raised(std::mem::take(&mut cx.out), String::new(), &e),
    }
}
