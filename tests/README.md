# K2SO Test Map

Everything that tests K2SO lives in one of **six** places. This doc
maps them so any future agent can answer "where do the tests live" and
"how do I run them" without grepping.

## Quick index

1. **Shell behavior** — this folder (`tests/`)
2. **Rust integration** — `crates/k2so-core/tests/` (16 files)
3. **Rust integration** — `crates/k2so-daemon/tests/` (15 files)
4. **Rust inline unit** — `#[cfg(test)] mod tests` in `crates/*/src/`
5. **Tauri inline unit** — `#[cfg(test)] mod tests` in `src-tauri/src/`
6. **TypeScript / Vitest** — `src/renderer/**/*.test.ts(x)`

---

## 1. Shell behavior tests — `tests/` (this folder)

Exercise the full running system over HTTP/filesystem. Each tier has
different prerequisites, listed below. All four scripts are
self-contained and print colored PASS/FAIL summaries.

| File | What it covers | Needs daemon running? | Needs workspace registered? |
|---|---|---|---|
| `behavior-test-tier3.sh` | Migrations, template content, CLI script correctness, Rust invariants via grep | no | no |
| `behavior-test-tier1.sh` | Filesystem state after CLI calls — lock files, CLAUDE.md scaffolds, event queue, triage summary | **yes** | no |
| `behavior-test-tier2.sh` | DB-dependent flows — source gating, workspace state, hook pipeline, scheduler-tick | **yes** | **yes** |
| `cli-integration-test.sh` | End-to-end CLI surface — every verb tested against a live daemon | **yes** | **yes** |

### Running

```bash
# Tier 3 — no setup needed. Start here on a cold machine.
bash tests/behavior-test-tier3.sh

# Tiers 1 / 2 / cli-integration — need a running daemon and, for 2 + cli,
# a registered test workspace.
#
# 1. Point TEST_WORKSPACE at a git-initialized dir. Default is
#    /Users/z3thon/DevProjects/k2so-cli-test.
# 2. Register that path in the projects table:
sqlite3 ~/.k2so/k2so.db "INSERT OR REPLACE INTO projects \
    (id, path, name, color, agent_mode, pinned, tab_order, heartbeat_mode) \
    VALUES ('cli-test', '$TEST_WORKSPACE', 'cli-test', \
    '#888', 'manager', 0, 99, 'heartbeat')"
# 3. Run whichever tier(s) you want:
bash tests/behavior-test-tier1.sh
bash tests/behavior-test-tier2.sh
bash tests/cli-integration-test.sh
```

### Baseline expectations (as of Phase 4 H7.3)

| File | Pass / Fail / Skip |
|---|---|
| tier3 | 385 / 0 / 4 |
| tier1 | 19 / 0 / 4 |
| tier2 | 16 / 0 / 0 |
| cli-integration | 111 / 0 / 0 |

The tier1/tier2 skips are all "project row not found in DB" branches
guarded on the test workspace not being registered. If you register
the workspace, the skips convert to real assertions.

---

## 2. Rust integration tests — `crates/k2so-core/tests/` (14 files)

Gated on the `session_stream` feature (most also need `test-util` for
the in-memory DB helper). Each file covers one subsystem in isolation
— no HTTP, no daemon process, just Rust modules calling each other.

| File | Tests | Subject |
|---|---:|---|
| `session_stream_routing.rs` | 22 | Delivery enum + roster + routing matrix (pure logic) |
| `session_stream_registry.rs` | 22 | SessionRegistry lifecycle + broadcast fanout |
| `session_stream_archive.rs` | 19 | NDJSON archive writer task |
| `session_stream_apc.rs` | 18 | APC extractor, all 8 verbs + split-mid-sequence |
| `session_stream_egress.rs` | 16 | Egress composer — four-cell matrix + audit |
| `session_stream_bus.rs` | 15 | Awareness bus singleton subscribe/publish |
| `session_stream_line_mux.rs` | 14 | LineMux vte::Perform state machine |
| `session_stream_claude_recognizer.rs` | 11 | Claude Code T0.5 box recognition |
| `session_stream_setting.rs` | 9 | `use_session_stream` project setting |
| `session_stream_inbox.rs` | 9 | Filesystem inbox write/drain + concurrency |
| `session_stream_pty.rs` | 8 | Real PTY session_stream spawn + dual-emit |
| `session_stream_types.rs` | 8 | Frame / Line / SemanticKind / Session round-trip |
| `session_stream_ingress.rs` | 7 | APC → egress pipeline end-to-end |
| `session_stream_awareness.rs` | 6 | AgentSignal / SignalKind serde |

### Running

```bash
# All core tests (lib + integration).
cargo test -p k2so-core --features session_stream,test-util

# Just one file.
cargo test -p k2so-core --features session_stream,test-util \
    --test session_stream_routing
```

---

## 3. Rust integration tests — `crates/k2so-daemon/tests/` (12 files)

Daemon-side HTTP handlers, WebSocket upgrades, and spawn/inject flows.
Each file corresponds to one feature area.

| File | Tests | Subject |
|---|---:|---|
| `terminal_routes_integration.rs` | 17 | `/cli/terminal/read,write,spawn,spawn-background,agents/running` |
| `watchdog_integration.rs` | 6 | Daemon harness-watchdog escalation ladder |
| `sessions_ws_integration.rs` | 6 | `/cli/sessions/subscribe` WS loopback |
| `agents_routes_integration.rs` | 5 | `/cli/agents/launch,delegate` (H5) |
| `triage_integration.rs` | 4 | Triage read-only summary + scheduler-fire dispatch |
| `scheduler_wake_integration.rs` | 4 | `DaemonWakeProvider` auto-launch |
| `companion_routes_integration.rs` | 4 | `/cli/companion/sessions,projects-summary` (H4) |
| `spawn_to_signal_e2e.rs` | 3 | HTTP spawn → signal → inject end-to-end |
| `pending_live_durability.rs` | 3 | F3 pending-live queue + boot replay |
| `providers_inject_integration.rs` | 2 | `DaemonInjectProvider` → real PTY |
| `awareness_ws_integration.rs` | 2 | `/cli/awareness/subscribe` WS loopback |
| `heartbeat_port_claim_integration.rs` | 1 | Daemon eagerly owns heartbeat.port (H7) |
| `heartbeat_fire_v2_integration.rs` | (varies) | 0.37.0 v2-retirement: wake_headless registers in v2_session_map + writes workspace_sessions row |
| `workspace_msg_integration.rs` | 7 | 0.37.0 simplified-msg: `workspace_msg::resolve_workspace` (name/path/UUID) + `deliver_to_inbox` |

### Running

```bash
# All daemon tests.
cargo test -p k2so-daemon

# Just one file.
cargo test -p k2so-daemon --test terminal_routes_integration
```

---

## 4. Inline unit tests — `crates/*/src/**/*.rs`

Standard Rust convention: `#[cfg(test)] mod tests { ... }` blocks sit
next to the code they test. **Not in a separate folder.** To find
them, grep:

```bash
# List every file with inline tests.
grep -rl "^#\[cfg(test)\]" crates/*/src/ | sort -u

# List every inline test name.
grep -rn "^    fn test_\|^    #\[test\]\|^    async fn" crates/*/src/
```

Current counts (Phase 4):

| Crate | Inline unit tests |
|---|---:|
| `k2so-core` | 275 |
| `k2so-daemon` | 11 |

Running them:

```bash
# Core unit tests only (skips integration).
cargo test -p k2so-core --features session_stream,test-util --lib

# Daemon unit tests only.
cargo test -p k2so-daemon --lib
```

---

## 5. Tauri inline unit tests — `src-tauri/src/**/*.rs`

The host-side Tauri binary ships its own `#[cfg(test)] mod tests`
blocks for command logic that doesn't live in `k2so-core`. These run
under the `k2so` crate (NOT `k2so-daemon` or `k2so-core`).

Notable suites:
- `commands::k2so_agents::migration_safety_tests` — 0.37.0 unification
  migration: idempotency on second launch, teardown safety, drift
  adoption.
- `commands::*::tests` — per-command unit assertions for whatever the
  command's pure helpers handle (path resolution, frontmatter parsing,
  etc.).

```bash
# All Tauri lib tests.
cargo test --release -p k2so --lib
```

Current count (0.37.0): **76 passing**.

---

## 6. TypeScript / Vitest — `src/renderer/**/*.test.ts(x)`

Renderer-side tests for Zustand stores, pure helpers, and React
components. Configured via `vitest.config.ts`; runs under jsdom.

Files:
- `src/renderer/stores/tabs.test.ts` — tab store invariants
- `src/renderer/lib/smoke.test.ts` — smoke
- `src/renderer/lib/terminal-id.test.ts` — terminal id parsing
- `src/renderer/kessel/{client,grid,selection,style,config}.test.ts` — Kessel renderer pieces
- `src/renderer/kessel/SessionStreamView.test.tsx` — component test

```bash
# Run all vitest.
bun run test

# Type-check separately (catches drift even without running tests).
bunx tsc --noEmit
```

Current counts (0.37.0): 133 passing, 2 known-pre-existing failing
(`config.test.ts` uses `bun:test` import vitest can't resolve;
`SessionStreamView.test.tsx` has DOM-layout assertions that depend on
jsdom behavior the runtime doesn't replicate).

---

## Full battery, one-liner

From a clean, registered workspace with the daemon running:

```bash
# Rust everything (core + daemon + Tauri lib).
cargo test --release --workspace

# TS type-check + vitest.
bunx tsc --noEmit && bun run test

# Shell suites that work without manual setup.
bash tests/behavior-test-tier3.sh
bash tests/behavior-test-tier1.sh
bash tests/cli-integration-test.sh

# Tier 2 needs the test workspace registered in K2SO UI first
# (see "Running" under section 1).
bash tests/behavior-test-tier2.sh
```

Current totals (0.37.0):
- Rust core lib: 302 passing
- Rust core integration (session_stream_*): all green
- Rust daemon integration: ~80 passing across 15 files (incl. new `workspace_msg_integration.rs`)
- Tauri lib: 76 passing
- Vitest: 133 passing (2 known pre-existing failures, see section 6)
- Shell tier 1: 19 passing
- Shell tier 3: 385 passing
- Shell CLI integration: 110 passing

---

## How to wire a new test

**New behavior / user-visible flow** → add to the relevant tier. Most
go to tier1 (filesystem) or tier2 (DB). Full-surface regressions go in
`cli-integration-test.sh`.

**New core module** → add an integration file in
`crates/k2so-core/tests/` named `session_stream_<module>.rs` (mirror
the naming convention) with `#![cfg(feature = "session_stream")]` at
the top.

**New daemon HTTP handler** → add an integration file in
`crates/k2so-daemon/tests/` named `<feature>_integration.rs`. Use the
`static TEST_LOCK: std::sync::Mutex<()>` pattern (see any existing
daemon test) to serialize against the shared session_map + DB
singletons.

**Unit-level invariant inside a module** → inline
`#[cfg(test)] mod tests { ... }` at the bottom of the `.rs` file.
Don't create a parallel `<name>_test.rs` sibling — the inline form is
the project convention.
