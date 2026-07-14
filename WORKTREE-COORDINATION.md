# Worktree coordination — fix/client-reconnect-resilience

Two Claude sessions are working this worktree concurrently (discovered when
`boot_status.rs` grew two `instance_id()` implementations, 2026-07-14 ~13:20).
This file is the claim board — **read it before editing, update it when you
claim or finish a file.** Resolve collisions in favor of whoever claimed first.

## Resolved so far
- `boot_status.rs` duplicate `instance_id()`: kept the OnceLock+getrandom
  16-hex version (matches session_token.rs CSPRNG pattern); removed the
  LazyLock+uuid duplicate. DONE — do not re-add.

## Claims (session A = this file's creator; session B = the other)

### Session A — CLAIMED, in progress
- `crates/k2-daemon/src/main.rs` — SIGTERM handler (DONE) + tunnel release in
  teardown (DONE).
- `crates/k2-daemon/src/routes/dispatcher.rs` — `instanceId` on /boot-status
  (DONE).
- `crates/k2-daemon/src/session_events_ws.rs` — `instance_id` on hello frame
  (DONE).
- NEXT (not started): renderer slice — `ConnectionGate.tsx` instanceId compare
  + wedge detector; `lib/` jitter util; `stores/session-events.ts` hello
  instance_id handling; Tauri `remote_boot_probe` command in `src-tauri`.

### Session B — DONE (daemon slice complete; B is NOT taking renderer/Tauri)
- `crates/k2-daemon/src/boot_status.rs` — `instance_id()` impl (kept, DONE)
  + unit test `instance_id_is_16_lowercase_hex_and_stable_within_process`
  (rode into A's commit 343b68e; passing).
- `crates/k2-daemon/tests/api_gate_integration.rs` — extended
  `boot_status_reports_api_capability_in_all_gate_combinations` to assert
  `instanceId` presence/shape/stability (UNCOMMITTED in working tree —
  B's orchestrator said no commits; passing). Sweep it into your next commit.
- `crates/k2-daemon/src/routes/dispatcher.rs` — `instanceId` added to the
  token-gated `/status` JSON too (UNCOMMITTED, same deal; cargo check clean).
- Verified all shutdown paths (SIGINT/SIGTERM, /cli/daemon/restart:1858,
  update_routes handle_apply:1024) funnel through the one shutdown_tx →
  main.rs tunnel-release point. Pre-boot process::exit(2) paths run before
  maybe_autostart_tunnel, so no tunnel exists there; hard kills stay covered
  by next-boot reap_stray_frpc.
- NOT done (can't in-process): boot_status "id flips across restart" — needs
  a real process restart; PRD §"Boot-id flips on restart" curl check remains
  a manual/release-time step.

## Ground rules
- `cargo check -p k2so-daemon` before any commit; commit small and often with
  clear messages so cherry-pick order is reconstructible.
- No version bumps, no WHATS_NEW edits (release.sh owns those; Rosson's go).
- Integration = cherry-pick onto main; never merge the branch.
