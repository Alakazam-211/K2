//! The `mpc-tools` binary: [`mpc_tools::run`] on the process's own arguments.

use std::io::Write;

fn main() {
    let ctx = mpc_tools::Ctx::from_process();
    let out = mpc_tools::run(std::env::args_os(), &ctx);
    // a closed pipe on either stream is not the tool's failure: its exit code still says how it went
    let _ = std::io::stdout().write_all(&out.stdout);
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().write_all(&out.stderr);
    std::process::exit(out.code);
}
