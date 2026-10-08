# Testing K2 (Rust crates)

How to write `k2-core` / `k2-daemon` tests that stay green in the Linux gate.
The rules come from the 0.45.1 quiet-gate work: every rule below is here because
a test broke it and flaked or failed on the gate.

## The gate

`scripts/test-gate-linux.sh <sha>` exports `<sha>` with `git archive` into a fresh
dir, builds with its own target dir, and runs `cargo test -p k2-core` and
`cargo test -p k2-daemon` (`--no-fail-fast`) under `env -i`: a temp `HOME`, a temp
`TMPDIR`, no `SHELL`, no `K2_*` / `K2SO_*`. It runs **as root**. It is green only
when every test binary reports 0 failed and the ignored tests are exactly its
allowlist. Write tests that pass there, as a normal user on a Mac, in any order,
on any thread count.

## Rules

1. **Fail loudly.** No try/catch swallowing, no `unwrap_or` defaults inside
   assertions, no skip-if-missing, no retries. An `#[ignore]`d test must be on the
   gate allowlist with a reason.
2. **Never touch the real machine.** No real `~/.k2`, `~/.claude`, `~/.codex`,
   keychain or production daemon (`test_isolation` panics if a test can reach it).
   Run tests with every `K2_*` / `K2SO_*` session variable cleared.
3. **Env changes go through `k2_core::test_env`** (in k2-daemon too; it is the
   same lock):
   - `TempHome::new()` for a temp `$HOME` (`TempHome::short()` when the test binds
     Unix sockets under `$HOME`: socket paths must fit `SUN_LEN`).
   - `EnvVar::set(k, v)` / `EnvVar::remove(k)` for any other variable.
   - `test_env::lock()` to hold the env still without changing it.

   The lock is re-entrant (nesting is fine) and never poisons. Guards restore the
   old value on drop, including on panic. Bind them: `let _home = TempHome::new();`,
   never `let _ = …` (that drops at once).
   **Never call `std::env::set_var` / `remove_var` in tests.** A ratchet test in
   each crate (`no_new_raw_env_mutation_in_k2_core` / `…_in_k2_daemon`) fails on
   any new raw call, and on any raw `HOME` / `PATH` / `SHELL` call at all.
4. **A test that reads `$HOME` holds the lock.** Anything that goes through
   `dirs::home_dir()` or a store under `~/.k2` (vaults, outboxes, the skin store,
   connect users) needs a `TempHome` for the whole time it depends on that value;
   otherwise another test's swap moves it mid-test.
5. **Lock order: env lock first.** Take `TempHome` / `EnvVar` / `test_env::lock()`
   *before* any module lock (`sql_server_test_lock`, `clear_jobs_for_test`,
   `mail_server_test_lock`, a file's `TEST_LOCK`), and before the DB lock.
6. **The shared test DB is shared.** `db::shared()` in tests is ONE in-memory DB
   for the whole test binary. A test that asserts on a whole table (empty, exact
   set, count) or runs a whole-table sweep (`backfill_workspace_handles`, orphan
   purge) takes `let _db = db::scoped_for_test();` first: a fresh, migrated DB for
   that thread. Code under test that hops threads still sees the shared DB; pass
   `guard.handle()` explicitly there. On the shared DB, use unique ids (uuid) and
   per-row helpers such as `workspace::handle::backfill_project_handle(conn, id)`.
7. **No agent CLIs.** Test builds refuse to resolve `claude`, `codex`, `grok`,
   `gemini`, `cursor-agent`, … through `PATH`. A test that makes the daemon spawn an
   agent (including a workspace's default agent) installs shims:
   `let _agents = test_env::AgentShim::install();` (each name runs `exec cat`).
8. **No dead well-known ports.** Never point a client at `127.0.0.1:9` / `:1` as
   "unreachable": where loopback RSTs are dropped, that connect hangs. Use
   `test_env::ErrorHttpServer::start(503)` (a real listener that answers an error),
   your own mock server, or `test_env::ClosedPort::new()` / `::dual()` (a reserved,
   non-listening port: refused at once). Never bind a listener and drop it to get
   a "closed" port: a parallel test can re-bind it before you connect.
9. **No chmod-based failure injection.** Root ignores `0444` / `0555`. Inject I/O
   errors structurally: a directory where a file must be written (EISDIR), a
   regular file where a directory must be (ENOTDIR), or a seam such as
   `connect_users::sessions_store_tmp_path()`.
10. **Unique temp paths.** `test_env::unique_temp_path(tag)` (uuid) or a fresh
    `TempHome`; never pid + nanos or `ThreadId` (they repeat across the test
    binaries of one run), never a fixed `/tmp/…` literal.
11. **No process-global config flips.** Code read from env once (spawn queue,
    perf gate) gets a per-thread or guarded test override
    (`spawn_queue::test_override`, `perf`'s gate override); tests never flip the
    env var under other tests.
12. **Write scripts you exec with `test_env::write_executable`.** `fs::write` +
    `chmod` + exec flakes with ETXTBSY ("Text file busy") on Linux: another test
    thread's `fork` can hold your write fd open at the moment you exec.
13. **Assert exactly.** Prefer an exact expected value over "different and longer"
    (`path_enrichment_tests` asserts the exact PATH from a fake login shell).

## Running locally

```sh
for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset $v; done
cargo test -p k2-core --lib -- <module>::     # one module
cargo test -p k2-daemon --test <file>        # one integration file
```

A route arm in `routes/dispatcher.rs` that answers on a keep-alive socket must
consume the peeked request on EVERY branch (GET included), or the loop answers
it again (the `GET /cli/tunnel/config` bug).

The nightly soak, `scripts/test-soak-linux.sh <sha>`, loops the known-flaky
groups 200x under CPU load, runs both unit binaries with shuffle seeds 1-10, and
runs the whole suite once under load.

Loop a flake fix before calling it fixed: run the module filter 200 times with
`--test-threads=12` under CPU load, and once with a shuffled order
(`RUSTC_BOOTSTRAP=1 <test-binary> -Z unstable-options --shuffle-seed N`).

`k2-daemon`'s binary is a shim over the library (`daemon_main::run`), so every
daemon unit test lives in, and runs once from, the library.
