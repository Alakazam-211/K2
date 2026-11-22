//! `k5-spec-fixture`: a test binary, never shipped (every build that ships
//! names `--bin k5-cli`). It runs the spec runner ([`k5_cli::spec::run`])
//! over the rows in `table.rs`, which contract-gen renders from
//! `crates/k5-cli/tests/data/spec_fixture.entries`: an entry no `k2
//! --schema` prints and no route of the real binary reaches.
//! `tests/spec_parity.rs` runs it against the shell CLI. A command line no
//! row starts, or one the runner hands back, goes to the shell CLI as the
//! front hands it over.

use k5_cli::fallthrough;
use std::ffi::OsString;

mod table;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let words: Option<Vec<&str>> = args.iter().map(|a| a.to_str()).collect();
    let row = words
        .as_deref()
        .and_then(|w| k5_cli::spec::find(table::SPEC, w));
    let exit = row
        .and_then(|row| k5_cli::spec::run(row, &args))
        .unwrap_or_else(|| fallthrough::hand_off(&args));
    std::process::exit(exit.code());
}
