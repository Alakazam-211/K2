//! A SOCKS4 or SOCKS4a proxy's reply as any bytes: an error or an open
//! tunnel, never a panic, and only reply code 90 opens it.
//!
//! Run from this folder: `cargo +nightly fuzz run socks4`.
#![no_main]

use libfuzzer_sys::fuzz_target;

const TARGETS: [&str; 3] = ["h.example", "127.0.0.5", "[::1]"];

fuzz_target!(|data: &[u8]| {
    let Some((pick, answer)) = data.split_first() else {
        return;
    };
    let target = TARGETS[usize::from(*pick) % TARGETS.len()];
    if proxy_env::fuzzing::socks4_reply(answer, target).is_ok() {
        assert!(answer.len() >= 8 && answer[1] == 90, "opened on {answer:?}");
    }
});
