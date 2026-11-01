k2-ptyd is a small per-user PTY host: it runs terminal programs in its own long-lived process, so they keep running when the program that drives them restarts, updates or crashes.

# k2-ptyd

> Internal name. This crate will be published as **kessel-pty**, part of the
> Kessel family of open-source terminal crates by Alakazam Labs. It isn't
> published yet (`publish = false`).

## What it does

- **Owns the PTYs.** An owner process (in K2, the daemon) asks ptyd to spawn
  programs on fresh PTYs. ptyd is never the owner's child (double fork +
  `setsid`), so restarting or unloading the owner's service doesn't take the
  programs with it.
- **Never loses output.** ptyd always reads every PTY master into a
  per-session ring with monotonic byte offsets. A program never blocks on
  output while nobody is attached, and a client can re-attach from any offset
  still in the ring.
- **Hands sessions over.** A new owner connects, takes over, lists the running
  and exited sessions and re-attaches where the old one left off. Listening
  sockets and other fds can be held by name across the gap.
- **Upgrades in place.** `upgrade` checks a new binary (sha256, `--selftest`),
  then `exec`s it with the full state handed over: same pid, same sessions,
  same socket.
- **Stays small.** It's one thread and one `poll(2)` loop. It depends on
  `libc`, `serde`, `serde_json`, `hmac`, `sha2` and `getrandom` only: no async
  runtime, no terminal emulator, no network or TLS crates.

## Security model

ptyd serves one user. Its control socket lives in a private (0700) directory.
The server checks the peer's uid and an HMAC-SHA256 challenge over a 0600 key
file that a fresh instance mints each time. The client checks the socket
directory's owner and mode and the server's uid. ptyd runs non-dumpable on
Linux, launches with an allowlisted environment, closes every other fd in
spawned children and resets their signal state. It never logs PTY bytes or
environment values.

## Usage

```text
k2-ptyd --launch                      start detached; exit 0 once listening (3 if already running)
k2-ptyd --serve                       run in the foreground (tests, debugging)
k2-ptyd --version
k2-ptyd --selftest --state-format N   can this build take over a running ptyd's state?
```

The state home (socket, key, lock, log) comes from `K2_HOME`, and the orphan
timeout from `K2_PTYD_ORPHAN_SECS`. These names are K2's for now. They'll
become configuration before the crate is published under its own name.

The library half has the wire protocol (`proto`), the authentication
(`auth`), the path layout (`paths`), the ring (`ring`) and a small blocking
client (`client`) for owners written in Rust.

## Platforms

macOS and Linux. Windows isn't supported yet.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
