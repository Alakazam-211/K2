//! `k5-cli`: the k5 command line's front. It answers `--version` itself
//! and hands every other verb to the shell CLI it carries
//! ([`k5_cli::fallthrough`]).
//!
//! The result goes to stdout and the front's own errors to stderr
//! ([`k5_cli::out`]); the exit codes are [`k5_cli::exit::Exit`]'s. The
//! notice for callers of `k2` ([`k5_cli::notice`]) is off: nothing here
//! prints it.

use k5_cli::called_as::CalledAs;
use k5_cli::exit::Exit;
use k5_cli::front::{self, Route};
use k5_cli::{embedded, fallthrough, out};
use std::ffi::OsString;

fn main() {
    let mut argv = std::env::args_os();
    let argv0 = argv.next().unwrap_or_default();
    let args: Vec<OsString> = argv.collect();
    let who = CalledAs::current(&argv0);
    let exit = match front::route(&args) {
        Route::Version => match out::stdout(&front::version_line(embedded::version())) {
            Ok(()) => Exit::Ok,
            Err(err) => out::after_stdout_error(&err),
        },
        Route::Fallthrough => {
            let err = fallthrough::run(who, &args);
            out::stderr_line(&err.line(who));
            err.exit()
        }
    };
    std::process::exit(exit.code());
}
