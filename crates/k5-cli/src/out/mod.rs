//! What the command line prints, and where.
//!
//! stdout carries only the result, so `out=$(k5 ...)` holds the answer and
//! nothing else. Errors, hints and notices go to stderr. Every write is
//! flushed at once, so the two streams come out in the order they were
//! written.
//!
//! A broken pipe on stdout (the reader went away: `k5 --version | true`)
//! is not a panic. The process ends as the shell CLI's does there: killed
//! by SIGPIPE ([`after_stdout_error`]).

pub mod json;

use crate::exit::Exit;
use std::io::{self, Write};

/// Writes `bytes` to `w` and flushes it.
pub fn write_flushed(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    w.write_all(bytes)?;
    w.flush()
}

/// Writes `text` to stdout as it is, flushed.
pub fn stdout(text: &str) -> io::Result<()> {
    write_flushed(&mut io::stdout().lock(), text.as_bytes())
}

/// Writes `text` and a newline to stdout, flushed.
pub fn stdout_line(text: &str) -> io::Result<()> {
    stdout(&format!("{text}\n"))
}

/// Writes `text` and a newline to stderr, flushed. A failure is dropped:
/// stderr is where it would have been told.
pub fn stderr_line(text: &str) {
    let _ = write_flushed(&mut io::stderr().lock(), format!("{text}\n").as_bytes());
}

/// The exit after a write to stdout failed.
///
/// A broken pipe ends the process by SIGPIPE, with nothing on stderr. The
/// shell CLI dies by SIGPIPE there too, silently on Linux; macOS's
/// `/bin/bash` 3.2 lets the script's exit trap catch the signal, so its
/// echo first prints `echo: write error: Broken pipe` with the script's
/// path and line, which the front doesn't copy. Rust ignores SIGPIPE
/// from the start, so this puts the default back and raises it. Were
/// SIGPIPE blocked, the raise does nothing and this returns
/// [`Exit::Error`], which is what bash exits with when SIGPIPE is ignored.
///
/// Any other failure is [`Exit::Error`], as before.
pub fn after_stdout_error(err: &io::Error) -> Exit {
    if err.kind() == io::ErrorKind::BrokenPipe {
        die_of_sigpipe();
    }
    Exit::Error
}

#[cfg(unix)]
fn die_of_sigpipe() {
    // SAFETY: both take plain constants; no handler of ours is installed
    // or runs.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        libc::raise(libc::SIGPIPE);
    }
}

/// Windows has no SIGPIPE: the write's failure is the exit.
#[cfg(not(unix))]
fn die_of_sigpipe() {}

#[cfg(test)]
mod tests {
    use super::*;

    struct Failing(io::ErrorKind, bool);

    impl Write for Failing {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.1 {
                Ok(buf.len())
            } else {
                Err(io::Error::from(self.0))
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(self.0))
        }
    }

    #[test]
    fn a_write_is_whole_and_flushed() {
        let mut w = io::BufWriter::with_capacity(64, Vec::new());
        write_flushed(&mut w, b"k2 0.46.15\n").unwrap();
        assert_eq!(w.buffer(), b"", "left in the buffer");
        assert_eq!(w.get_ref(), b"k2 0.46.15\n");
    }

    #[test]
    fn a_failed_write_or_flush_is_an_error() {
        let err = write_flushed(&mut Failing(io::ErrorKind::BrokenPipe, false), b"x").expect_err("write fails");
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
        let err = write_flushed(&mut Failing(io::ErrorKind::StorageFull, true), b"x").expect_err("flush fails");
        assert_eq!(err.kind(), io::ErrorKind::StorageFull);
    }

    /// The test process, like the binary, has SIGPIPE ignored by Rust, so
    /// a closed reader is a `BrokenPipe` error and not a signal. That is
    /// the error [`after_stdout_error`] acts on.
    #[cfg(unix)]
    #[test]
    fn a_pipe_with_no_reader_is_a_broken_pipe() {
        let (reader, mut writer) = io::pipe().unwrap();
        drop(reader);
        let err = write_flushed(&mut writer, b"k2 0.46.15\n").expect_err("no reader");
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn another_failure_is_exit_1() {
        for kind in [io::ErrorKind::StorageFull, io::ErrorKind::PermissionDenied, io::ErrorKind::Other] {
            assert_eq!(after_stdout_error(&io::Error::from(kind)), Exit::Error, "{kind:?}");
        }
    }
}
