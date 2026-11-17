//! `k5-cli`: the k5 command line's front. It answers `--version` itself
//! and hands every other verb to the shell CLI it carries
//! ([`k5_cli::fallthrough`]).

use k5_cli::called_as::CalledAs;
use k5_cli::front::{self, Route};
use k5_cli::{embedded, fallthrough};
use std::ffi::OsString;
use std::io::Write;

fn main() {
    let mut argv = std::env::args_os();
    let argv0 = argv.next().unwrap_or_default();
    let args: Vec<OsString> = argv.collect();
    let who = CalledAs::current(&argv0);
    let code = match front::route(&args) {
        Route::Version => {
            let line = front::version_line(embedded::version());
            let mut out = std::io::stdout().lock();
            match out.write_all(line.as_bytes()).and_then(|()| out.flush()) {
                Ok(()) => 0,
                Err(_) => 1,
            }
        }
        Route::Fallthrough => {
            let err = fallthrough::run(who, &args);
            eprintln!("{}", err.line(who));
            err.exit_code()
        }
    };
    std::process::exit(code);
}
