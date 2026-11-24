//! A SOCKS5 proxy's replies as any bytes (the method it picks, the user
//! and password status, the CONNECT reply and its bound address of any
//! type): an error or an open tunnel, never a panic.
//!
//! Run from this folder: `cargo +nightly fuzz run socks5`.
#![no_main]

use libfuzzer_sys::fuzz_target;

const TARGETS: [&str; 3] = ["h.example", "127.0.0.5", "[::1]"];

fuzz_target!(|data: &[u8]| {
    let Some((pick, answer)) = data.split_first() else {
        return;
    };
    let target = TARGETS[usize::from(*pick >> 1) % TARGETS.len()];
    let _ = proxy_env::fuzzing::socks5_replies(answer, pick & 1 == 1, target);
});
