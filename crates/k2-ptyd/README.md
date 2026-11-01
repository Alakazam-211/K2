k2-ptyd is a small per-user PTY host: it runs terminal programs in its own long-lived process, so they keep running when the program that drives them restarts, updates or crashes.

# k2-ptyd

> Internal name. This crate will be published as **kessel-pty**, part of the
> Kessel family of open-source terminal crates by Alakazam Labs. It isn't
> published yet (`publish = false`).

## What it does

- **Owns the PTYs.** The *owner* (see [Terms](#terms)) asks ptyd to spawn
  programs on fresh PTYs. ptyd is never the owner's child (double fork +
  `setsid`), so restarting or unloading the owner's service doesn't take the
  programs with it.
- **Never loses output.** ptyd always reads every PTY master into a
  per-session ring with monotonic byte offsets. A program never blocks on
  output while nobody is attached, and a client can re-attach from any offset
  still in the ring.
- **Hands sessions over.** A new owner connects, takes over, lists the running
  and exited sessions and re-attaches where the old one left off. Listening
  sockets and other fds can be held by name across *the gap*.
- **Upgrades in place.** `upgrade` checks a new binary (sha256, `--selftest`),
  then `exec`s it with the full state handed over: same pid, same sessions,
  same socket.
- **Stays small.** It's one thread and one `poll(2)` loop. It depends on
  `libc`, `serde`, `serde_json`, `hmac`, `sha2` and `getrandom` only: no async
  runtime, no terminal emulator, no network or TLS crates.

## How the crates fit

Three crates make a terminal that runs on a server and that any number of
viewers can watch and type into over a websocket:

1. **kessel-pty** (`k2-ptyd` until the rename) keeps the programs alive: it
   owns their PTYs in its own process and keeps their output in a ring.
2. Their output feeds an [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal)
   `Term`, which runs the terminal emulation.
3. **kessel-grid** (`k2-term-grid`) turns that `Term` into snapshots and
   deltas (and reflows them to another width).
4. **kessel-wire** (`k2-term-wire`) encodes the snapshots and deltas as
   compact binary websocket messages.

Each crate works on its own. kessel-crdt and kessel-sync are unrelated
sibling crates in the Kessel family; nothing here depends on them.

Used in [K2](https://k2.dev) (Alakazam Labs), an AI workspace tool, for its
terminal and agent sessions.

## Example

Launch ptyd, connect as the owner, spawn bash, attach from offset 0 and read.
The full program is [`examples/spawn_bash.rs`](examples/spawn_bash.rs), which
builds with the crate's tests:

```text
cargo build -p k2-ptyd --bins
cargo run -p k2-ptyd --example spawn_bash
```

Its core:

```rust,ignore
use std::io::Read;
use k2_ptyd::{client::Client, launch, paths::Paths, proto::{Reply, Request}};

// 1. Launch: double-forks the server out of this process, returns once it listens.
let home = std::env::temp_dir().join("ptyd-example");
std::fs::create_dir_all(&home)?;
launch::spawn_detached(&exe /* path to the k2-ptyd binary */, &home, &[])?;

// 2. Connect as the owner.
let mut owner = Client::connect_owner(&Paths::from_home(&home), "example-0.1.0", true)?;

// 3. Spawn bash on a fresh 80x24 PTY. The child gets exactly `env`, nothing inherited.
owner.request(Request::Spawn {
    sid: "demo".into(),
    argv: vec!["/bin/bash".into(), "--noprofile".into(), "--norc".into(), "-c".into(), "echo hello from ptyd".into()],
    env: vec![("PATH".into(), "/usr/bin:/bin".into()), ("TERM".into(), "xterm-256color".into())],
    cwd: None, cols: 80, rows: 24, ring_bytes: None, meta: serde_json::json!({}),
})?;

// 4. Wait for the exit event. The output is still in the session's ring.
while !matches!(owner.next_event()?, Reply::Exited { .. }) {}

// 5. Attach from offset 0: the stream replays the ring from its start
//    (for an exited session it ends after the backlog).
let mut out = Vec::new();
owner.attach("demo", 0)?.read_to_end(&mut out)?;
print!("{}", String::from_utf8_lossy(&out)); // hello from ptyd

// 6. Stop this ptyd.
owner.request(Request::Shutdown { end_sessions: true })?;
```

## Terms

- **Owner.** The single control client: the program that drives the
  sessions (for example a terminal server). It connects with the `owner`
  role, spawns, resizes, signals and kills sessions, and receives events
  (`exited`; `overrun` when an attached stream fell so far behind that the
  ring overwrote bytes it hadn't sent; `orphan_warning`). Only one owner is connected at a
  time: a new owner that connects with `takeover` evicts the old one (which
  gets `evicted`); without `takeover` it's refused as `busy`. Every other
  connection is a **data** connection: one byte stream for one session,
  opened with `attach { sid, from }`.
- **The gap.** The time between one owner going away (it restarts, updates or
  crashes) and the next one taking over. During the gap the sessions keep
  running, their output keeps going into the rings (once a ring is full its
  oldest bytes drop; `list` reports each ring's start and end offsets, and
  `attached` says where a stream really starts), exits are recorded, and fds
  held by name stay open. If the gap outlasts the
  *orphan timeout* (default 900 s, minimum 120 s) while sessions are still
  running, ptyd ends them and exits. An owner that connects after half the
  timeout gets an `orphan_warning` with the seconds left. With no sessions
  and no owner, ptyd exits after 60 s idle.
- **`--state-format N`.** The format of the state that `upgrade` hands from
  the running ptyd to the new binary (sessions, rings, held fds, key). Before
  it `exec`s, the running ptyd runs `<new binary> --selftest --state-format N`
  with its own format number. A build that can read format N prints
  `ok <version> N` and exits 0; otherwise it prints why on stderr and exits 1,
  and the upgrade is refused with nothing changed. This build reads format 1.

## Usage

```text
k2-ptyd --launch                      start detached; return once the server listens
k2-ptyd --serve                       run in the foreground (tests, debugging)
k2-ptyd --version                     print `k2-ptyd <version> proto <n>`
k2-ptyd --selftest --state-format N   can this build take over a running ptyd's state?
```

The state home (socket, key, lock, log) comes from `K2_HOME`, and the orphan
timeout from `K2_PTYD_ORPHAN_SECS`. These names come from K2, the product
ptyd was built for. They'll become configuration before the crate is
published under its own name.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | `--launch`: a new server is listening. `--serve`: the server stopped cleanly (shutdown request, idle exit, orphan timeout, `SIGTERM`/`SIGINT`). `--version`, and `--selftest` when this build can read the state format. |
| 1 | Failure: the server couldn't start (see `<home>/logs/ptyd.log`), `K2_HOME` is missing or not absolute, or `--selftest` can't read that state format (or `--state-format N` is missing). Also every mode on Windows. |
| 2 | Usage error: an unknown mode or bad arguments. `K2_HOME` is checked first, so without it a bad mode exits 1. |
| 3 | `--launch` / `--serve`: another ptyd already owns this state home. An owner can treat 3 like 0: a server is listening. |

The library half has the wire protocol (`proto`), the authentication
(`auth`), the path layout (`paths`), the ring (`ring`) and a small blocking
client (`client`) for owners written in Rust.

## Security model

ptyd serves one user. Its control socket lives in a private (0700) directory.
The server checks the peer's uid and an HMAC-SHA256 challenge over a 0600 key
file that a fresh instance mints each time. The client checks the socket
directory's owner and mode and the server's uid. ptyd runs non-dumpable on
Linux, launches with an allowlisted environment, closes every other fd in
spawned children and resets their signal state. It never logs PTY bytes or
environment values.

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
