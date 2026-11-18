//! `k5-cli`: the k5 command line's front. It answers `--version`,
//! `--version --check`, `cli status` and the families in
//! [`k5_cli::families`] itself and hands every other verb to the shell CLI
//! it carries ([`k5_cli::fallthrough`]). Before a verb
//! falls through, the passport rule ([`k5_cli::passport`]) runs; for now
//! it only counts. The binary's own requests carry its verdict too.
//!
//! The result goes to stdout and the front's own errors to stderr
//! ([`k5_cli::out`]); the exit codes are [`k5_cli::exit::Exit`]'s. The
//! notice for callers of `k2` ([`k5_cli::notice`]) is off: nothing here
//! prints it. With `K5_CLI_TRACE_TIMING` set, each phase's time goes to
//! stderr ([`k5_cli::trace`]).

use k5_cli::called_as::CalledAs;
use k5_cli::exit::Exit;
use k5_cli::front::{self, Route};
use k5_cli::{check, cli_status, daemon, embedded, fallthrough, families, out, passport, trace};
use std::ffi::OsString;

/// Writes `text` to stdout; the exit after it.
fn print(text: &str, then: Exit) -> Exit {
    match out::stdout(text) {
        Ok(()) => then,
        Err(err) => out::after_stdout_error(&err),
    }
}

/// `--version --check`.
fn version_check(args: &[OsString]) -> Exit {
    let ours = embedded::version();
    if let Err(err) = out::stdout(&front::version_line(ours)) {
        return out::after_stdout_error(&err);
    }
    let client = daemon::current().map_err(|e| (e.to_string(), Exit::Error)).and_then(|s| {
        s.client(passport::current(args).would_refuse()).map_err(|e| (e.to_string(), Exit::DaemonUnreachable))
    });
    let outcome = match client {
        Ok(mut c) => check::run(ours, &mut c),
        Err((line, exit)) => check::Outcome::Failed(line, exit),
    };
    match outcome {
        check::Outcome::Answer(line) => print(&format!("{line}\n"), Exit::Ok),
        check::Outcome::Failed(line, exit) => {
            out::stderr_line(&line);
            exit
        }
    }
}

/// `cli status [--json]`.
fn cli_status(args: &[OsString], json: bool) -> Exit {
    let counts = match daemon::current() {
        Err(e) => cli_status::Counts::Unavailable(e.to_string()),
        Ok(s) => match s.client(passport::current(args).would_refuse()) {
            Ok(mut c) => cli_status::fetch(&mut c),
            Err(e) => cli_status::Counts::Unavailable(e.to_string()),
        },
    };
    let status = cli_status::status(k5_cli::route::TABLE, embedded::version(), counts);
    let text = if json { status.json() } else { status.table() };
    let exit = print(&text, Exit::Ok);
    if let Some(line) = status.note() {
        out::stderr_line(&line);
    }
    exit
}

/// The shell CLI runs the command line. The passport rule, count-only:
/// the shell is told when the call would be refused, and a value the
/// caller set never stands. Nothing else has changed the process
/// environment, so it is what the shell inherits.
fn shell(who: CalledAs, args: &[OsString]) -> Exit {
    match passport::hand_off_env(passport::current(args)) {
        (name, Some(value)) => std::env::set_var(name, value),
        (name, None) => std::env::remove_var(name),
    }
    trace::mark("passport rule");
    let err = fallthrough::run(who, args);
    out::stderr_line(&err.line(who));
    err.exit()
}

fn main() {
    trace::start();
    let mut argv = std::env::args_os();
    let argv0 = argv.next().unwrap_or_default();
    let args: Vec<OsString> = argv.collect();
    let who = CalledAs::current(&argv0);
    trace::mark("called_as");
    let route = front::route(&args);
    trace::mark("route");
    let exit = match route {
        Route::Version => print(&front::version_line(embedded::version()), Exit::Ok),
        Route::VersionCheck => version_check(&args),
        Route::CliStatus { json } => cli_status(&args, json),
        Route::CliHelp => print(cli_status::HELP, Exit::Ok),
        Route::Whoami => families::whoami::run(&args),
        // A form the family hands back goes to the shell before anything
        // is printed or sent.
        Route::Done => families::done::run(&args).unwrap_or_else(|| shell(who, &args)),
        Route::Respond => families::respond::run(&args).unwrap_or_else(|| shell(who, &args)),
        Route::Inbox => families::inbox::run(&args).unwrap_or_else(|| shell(who, &args)),
        Route::Read => families::read::run(&args).unwrap_or_else(|| shell(who, &args)),
        Route::Fallthrough => shell(who, &args),
    };
    trace::finish("command");
    std::process::exit(exit.code());
}
