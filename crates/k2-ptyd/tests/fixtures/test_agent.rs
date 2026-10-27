//! Fake agent for k2-ptyd tests (PRD §12 harness). Modes:
//! - `probe <out.json>`: record what the child sees (session, ctty, fds,
//!   signal dispositions + mask, cwd, env, IUTF8), print PROBE_DONE, sleep.
//! - `burst <mib>`: write <mib> MiB of `a..z` (byte i = b'a' + i % 26), exit 0.
//! - `seq`: print `seq=<n> ` every 10 ms forever.
//! - `echo`: for each input line print `got:<line>`.
//! - `hup-ignore`: ignore SIGHUP, print READY, sleep forever.
//! - `sleep`: print READY, sleep forever.
//! - `exit <code>`: print BYE, exit with <code>.

//!
//! `#![no_main]`: Rust's normal startup ignores SIGPIPE and installs
//! SIGSEGV/SIGBUS handlers before `main`, which would hide what ptyd handed
//! the child. With a C `main` the probe sees the inherited dispositions.
// k2-ptyd is Unix-only until its Windows port (0.47); this fake agent with it.
#![cfg_attr(unix, no_main)]

#[cfg(unix)]
#[no_mangle]
pub extern "C" fn main(_argc: libc::c_int, _argv: *const *const libc::c_char) -> libc::c_int {
    imp::run();
    0
}

#[cfg(not(unix))]
fn main() {
    eprintln!("k2-ptyd-test-agent: k2-ptyd is Unix-only until 0.47");
    std::process::exit(2);
}

#[cfg(unix)]
#[path = "test_agent_unix.rs"]
mod imp;
