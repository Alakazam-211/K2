//! An HTTP proxy's answer to CONNECT as any bytes: an error or an open
//! tunnel, never a panic, and only a 2xx status line opens it.
//!
//! Run from this folder: `cargo +nightly fuzz run connect_answer`.
#![no_main]

use libfuzzer_sys::fuzz_target;

const TARGETS: [&str; 3] = ["h.example", "127.0.0.5", "[::1]"];

fuzz_target!(|data: &[u8]| {
    let Some((pick, answer)) = data.split_first() else {
        return;
    };
    let target = TARGETS[usize::from(*pick) % TARGETS.len()];
    if proxy_env::fuzzing::http_connect_answer(answer, target).is_ok() {
        let line = answer.split(|b| *b == b'\n').next().unwrap_or(b"");
        assert!(
            line.len() >= 12 && line.starts_with(b"HTTP/1.") && line[9] == b'2',
            "a tunnel opened on {:?}",
            String::from_utf8_lossy(line)
        );
    }
});
