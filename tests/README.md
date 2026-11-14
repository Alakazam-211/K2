# K2 Test Map

Everything that tests K2 lives in one of **six** places. This doc
maps them so any future agent can answer "where do the tests live" and
"how do I run them" without grepping.

## Quick index

1. **Shell behavior** — this folder (`tests/`)
2. **Rust integration** — `crates/k2-core/tests/`
3. **Rust integration** — `crates/k2-daemon/tests/`
4. **Rust inline unit** — `#[cfg(test)] mod tests` in `crates/*/src/`
5. **Tauri inline unit** — `#[cfg(test)] mod tests` in `src-tauri/src/`
6. **TypeScript / Vitest** — `src/renderer/**/*.test.ts(x)`

---

## 1. Shell behavior tests — `tests/` (this folder)

Headless checks against a sandbox daemon (temp HOME, no setup) live in
`tests/cli/`; the hook pipeline's `k2 hooks status` check is
`tests/cli/hooks_status.sh`. Each script is self-contained and fails
loudly. The header of each script says what it needs built first.

### Running

```bash
cargo build -p k2-daemon && bash tests/cli/hooks_status.sh
```

---

## 2. Rust integration tests — `crates/k2-core/tests/`

Each `session_stream_*.rs` file covers one subsystem in isolation: no
HTTP, no daemon process, just Rust modules calling each other. Some
need the `test-util` feature for the in-memory DB helper; the crate's
own dev-dependency turns it on.

| File | Subject |
|---|---|
| `session_stream_routing.rs` | Delivery enum + roster + routing matrix (pure logic) |
| `session_stream_registry.rs` | SessionRegistry lifecycle + broadcast fanout |
| `session_stream_archive.rs` | NDJSON archive writer task |
| `session_stream_apc.rs` | APC extractor, all 8 verbs + split-mid-sequence |
| `session_stream_egress.rs` | Egress composer: four-cell matrix + audit |
| `session_stream_bus.rs` | Awareness bus singleton subscribe/publish |
| `session_stream_line_mux.rs` | LineMux vte::Perform state machine |
| `session_stream_claude_recognizer.rs` | Claude Code T0.5 box recognition |
| `session_stream_setting.rs` | `use_session_stream` project setting |
| `session_stream_inbox.rs` | Filesystem inbox write/drain + concurrency |
| `session_stream_types.rs` | Frame / Line / SemanticKind / Session round-trip |
| `session_stream_ingress.rs` | APC → egress pipeline end-to-end |
| `session_stream_awareness.rs` | AgentSignal / SignalKind serde |

### Running

```bash
# All core tests (lib + integration).
cargo test -p k2-core

# Just one file.
cargo test -p k2-core --test session_stream_routing
```

---

## 3. Rust integration tests — `crates/k2-daemon/tests/`

Daemon-side HTTP handlers, WebSocket upgrades, and spawn/inject flows.
Most files sit in one folder per area (`it_apps/`, `it_chat/`,
`it_compute/`, `it_contract/`, `it_guests/`, `it_mail/`, `it_misc/`,
`it_sessions/`, `it_zen/`). Each folder is one test binary and each
file in it is a module of that binary. Some examples:

| File | Subject |
|---|---|
| `it_sessions/agents_routes_integration.rs` | `/cli/agents/launch,delegate` |
| `it_sessions/triage_integration.rs` | Triage read-only summary + scheduler-fire dispatch |
| `it_sessions/awareness_ws_integration.rs` | `/cli/awareness/subscribe` WS loopback |
| `it_sessions/heartbeat_port_claim_integration.rs` | Daemon eagerly owns heartbeat.port |
| `it_sessions/heartbeat_fire_v2_integration.rs` | wake_headless registers in v2_session_map + writes workspace_sessions row |
| `it_chat/workspace_msg_integration.rs` | `workspace_msg::resolve_workspace` (name/path/UUID) |

### Running

The daemon test binaries must run under cargo-nextest (each test in
its own process); see `crates/k2-daemon/tests/support/env.rs`.

```bash
# All daemon tests.
cargo nextest run -p k2-daemon

# Just one area.
cargo nextest run -p k2-daemon --test it_sessions

# Just one file in it.
cargo nextest run -p k2-daemon --test it_sessions agents_routes_integration::
```

---

## 4. Inline unit tests — `crates/*/src/**/*.rs`

Standard Rust convention: `#[cfg(test)] mod tests { ... }` blocks sit
next to the code they test, or in a `<name>_tests.rs` beside it. **Not in a separate folder.** To find
them, grep:

```bash
# List every file with inline tests.
grep -rl "^#\[cfg(test)\]" crates/*/src/ | sort -u

# List every inline test name.
grep -rn "^    fn test_\|^    #\[test\]\|^    async fn" crates/*/src/
```

Running them:

```bash
# Core unit tests only (skips integration).
cargo test -p k2-core --lib

# Daemon unit tests only.
cargo test -p k2-daemon --lib
```

---

## 5. Tauri inline unit tests — `src-tauri/src/**/*.rs`

The host-side Tauri binary ships its own `#[cfg(test)] mod tests`
blocks for the pure helpers of its commands. These run under the `k2`
crate (NOT `k2-daemon` or `k2-core`). One example is
`commands::k2so_agents::pure_helper_tests`.

```bash
# All Tauri lib tests.
cargo test --release -p k2 --lib
```

---

## 6. TypeScript / Vitest — `src/renderer/**/*.test.ts(x)`

Renderer-side tests for Zustand stores, pure helpers, and React
components. Configured via `vitest.config.ts`.

Some of the files:
- `src/renderer/stores/tabs.test.ts` — tab store invariants
- `src/renderer/lib/smoke.test.ts` — smoke
- `src/renderer/lib/terminal-id.test.ts` — terminal id parsing
- `src/renderer/kessel/daemon-ws.test.ts` — daemon WebSocket URL helpers

```bash
# Run all vitest.
bun run test

# Type-check separately (catches drift even without running tests).
# tsc-rs, pinned; see docs/typecheck.md. Bare `tsc --noEmit` checks nothing here.
bun run typecheck:web
```

---

## Full battery, one-liner

Rust: run the per-crate commands in sections 2 to 5.

```bash
# TS type-check + vitest.
bun run typecheck:web && bun run test
```

---

## How to wire a new test

**New behavior / user-visible flow** → add a headless `tests/cli/`
script (sandbox daemon).

**New core module** → add an integration file in
`crates/k2-core/tests/`.

**New daemon HTTP handler** → add a `<feature>_integration.rs` file to
the matching `crates/k2-daemon/tests/it_*/` folder and name it in that
folder's `main.rs`. Tests that share process-wide state use the
`static TEST_LOCK` mutex pattern (see any existing daemon test).

**Unit-level invariant inside a module** → inline
`#[cfg(test)] mod tests { ... }` at the bottom of the `.rs` file. When
a test module grows to a large share of its file, move it to a sibling
`<name>_tests.rs` and name it from the file with
`#[cfg(test)] #[path = "<name>_tests.rs"] mod tests;` (see
`crates/k2-core/src/first_run_tests.rs`). The tests keep their access
to the module's private items either way.
