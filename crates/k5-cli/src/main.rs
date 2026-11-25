//! `k5-cli`: the k5 command line's front. It answers `--version`,
//! `--version --check`, `cli status` (and `cli` with any other word), the families in
//! [`k5_cli::families`], the spec verbs ([`k5_cli::spec`]), and the study
//! and help pages it can print as the shell does ([`k5_cli::pages`]) itself, and hands every other verb to
//! the shell CLI it carries ([`k5_cli::fallthrough`]). Before a verb
//! falls through, the passport rule ([`k5_cli::passport`]) runs; for now
//! it only counts. The binary's own requests carry its verdict too.
//!
//! The result goes to stdout and the front's own errors to stderr
//! ([`k5_cli::out`]); the exit codes are [`k5_cli::exit::Exit`]'s. Called
//! by an alias the brand file retires, the front first shows the notice
//! or the tombstone ([`k5_cli::notice`]); the default brand has none. With
//! `K5_CLI_TRACE_TIMING` set, each phase's time goes to stderr
//! ([`k5_cli::trace`]).

use k5_cli::called_as::CalledAs;
use k5_cli::exit::Exit;
use k5_cli::front::{self, Route};
use k5_cli::offline::{self, Env, Prelude};
use k5_cli::pages::{self, Page};
use k5_cli::{
    check, cli_status, codex_gate, daemon, embedded, fallthrough, notice, out, passport, port,
    spec, trace,
};
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
    let client = daemon::current()
        .map_err(|e| (e.to_string(), Exit::Error))
        .and_then(|s| {
            s.client(passport::current(args).would_refuse())
                .map_err(|e| (e.to_string(), Exit::DaemonUnreachable))
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

/// The shell CLI runs the command line ([`fallthrough::hand_off`]).
fn shell(args: &[OsString]) -> Exit {
    fallthrough::hand_off(args)
}

/// A help page the binary could not print (its gate): a native family's
/// help-word line goes to the family's own code first
/// ([`front::help_word_family`]), anything else to the shell.
fn after_help_gate(args: &[OsString]) -> Exit {
    match front::help_word_family(args) {
        Some(noun) => (noun.run)(args).unwrap_or_else(|| shell(args)),
        None => shell(args),
    }
}

/// A page: printed here when the shell's environment lets the binary
/// answer it ([`pages`]), else handed to the shell.
fn page(args: &[OsString], answer: impl FnOnce(&Prelude, &Env) -> Option<Page>) -> Exit {
    page_or(answer, || shell(args))
}

/// [`page`], with `otherwise` in place of the shell when the binary
/// can't print it.
fn page_or(
    answer: impl FnOnce(&Prelude, &Env) -> Option<Page>,
    otherwise: impl FnOnce() -> Exit,
) -> Exit {
    let env = Env::current();
    let found = offline::shell_pwd(&env)
        .and_then(|cwd| offline::prelude(&env, &cwd))
        .ok()
        .and_then(|p| answer(&p, &env));
    let Some(page) = found else {
        return otherwise();
    };
    if let Some(lead) = &page.lead {
        // As `echo ... >&2` under `set -e`: a stderr that can't be written
        // ends the run before the page.
        if let Err(err) = out::write_flushed(&mut std::io::stderr().lock(), lead.as_bytes()) {
            return out::after_stdout_error(&err);
        }
    }
    if page.stderr {
        // As `cat >&2`: a stderr that can't be written is not an error.
        let _ = out::write_flushed(&mut std::io::stderr().lock(), page.text.as_bytes());
        return page.exit;
    }
    print(&page.text, page.exit)
}

/// The arguments as words, when each is UTF-8 (the front routes only
/// such command lines to a page).
fn words(args: &[OsString]) -> Option<Vec<&str>> {
    args.iter().map(|a| a.to_str()).collect()
}

fn main() {
    trace::start();
    let mut argv = std::env::args_os();
    let argv0 = argv.next().unwrap_or_default();
    let args: Vec<OsString> = argv.collect();
    let who = CalledAs::current(&argv0);
    trace::mark("called_as");
    // An alias the brand file retires: a tombstone ends the call here, a
    // notice prints its line when due, then the command runs.
    if let Some(exit) = notice::on_call(who) {
        std::process::exit(exit.code());
    }
    // The shell's prelude checks the port before anything else runs: a
    // setting or saved port that isn't one stops every command, a native
    // verb's too.
    match port::check_current() {
        port::Check::Go => {}
        port::Check::Refuse(line) => {
            // As `printf ... >&2`: a stderr that can't be written changes
            // nothing.
            let _ = out::write_flushed(&mut std::io::stderr().lock(), &line);
            std::process::exit(Exit::Usage.code());
        }
        port::Check::Shell => std::process::exit(shell(&args).code()),
    }
    trace::mark("port");
    // The shell's Codex tab gate (`_cli_codex_tab_gate`), before the front
    // routes, prints or sends anything: a Codex call K2 can't confirm runs
    // only the help words. A stray `--` is refused first (the route).
    match codex_gate::check(&args) {
        codex_gate::Verdict::Go => {}
        codex_gate::Verdict::Stop(exit) => std::process::exit(exit.code()),
        codex_gate::Verdict::Shell => std::process::exit(shell(&args).code()),
    }
    trace::mark("codex gate");
    // A terminal of another brand's server: one line on stderr, then the
    // command runs (help and study too, which never ask the daemon).
    k5_cli::brand_check::session();
    let route = front::route(&args);
    trace::mark("route");
    // In an identity session a call that carries the session's passport
    // goes to the shell, which sends it over the session socket. The
    // binary's own verbs (version, cli) and a refused `--` stay here.
    if passport::identity_hand_off_here(&args)
        && !matches!(
            route,
            Route::Version
                | Route::VersionCheck
                | Route::CliStatus { .. }
                | Route::CliHelp
                | Route::CliWord
                | Route::NoDashdash
        )
    {
        std::process::exit(shell(&args).code());
    }
    let exit = match route {
        Route::Version => print(&front::version_line(embedded::version()), Exit::Ok),
        Route::VersionCheck => version_check(&args),
        Route::CliStatus { json } => cli_status(&args, json),
        Route::CliHelp => print(cli_status::HELP, Exit::Ok),
        Route::CliWord => {
            out::stderr_line(&cli_status::shell_line());
            Exit::Usage
        }
        // A family's code (its row's `run`, in its module): a form it
        // hands back goes to the shell before anything is printed or sent.
        Route::Family(noun) => (noun.run)(&args).unwrap_or_else(|| shell(&args)),
        Route::Spec(row) => spec::run(row, &args).unwrap_or_else(|| shell(&args)),
        Route::Study(s) => page(&args, |p, env| {
            pages::study_page(s, p, env, || passport::current(&args).would_refuse())
        }),
        Route::Help(form) => page_or(
            |p, env| pages::help_page(form, p, env, || passport::current(&args).would_refuse()),
            || after_help_gate(&args),
        ),
        Route::Daily => page(&args, |p, env| {
            pages::daily_page(p, env, || passport::current(&args).would_refuse())
        }),
        Route::Composed(c) => page(&args, |p, env| {
            let argv = words(&args)?;
            pages::composed_page(c, &argv, p, env, || passport::current(&args).would_refuse())
        }),
        Route::Unknown => page(&args, |p, env| {
            let argv = words(&args)?;
            pages::unknown_page(&argv, p, env, || passport::current(&args).would_refuse())
        }),
        Route::NoDashdash => {
            if let Some(line) = front::no_dashdash_line(&args) {
                out::stderr_line(&line);
            }
            Exit::Usage
        }
        Route::Fallthrough => shell(&args),
    };
    trace::finish("command");
    std::process::exit(exit.code());
}
