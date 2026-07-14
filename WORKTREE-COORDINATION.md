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

### Session B — observed so far
- `crates/k2-daemon/src/boot_status.rs` — `instance_id()` impl (kept, DONE).
- If you're taking more: CLAIM IT HERE before editing, or work files not
  listed above. Suggested clean split: B takes the daemon-side tests
  (`boot_status` flip test, teardown test) + `update_routes.rs` verification;
  A takes renderer + Tauri probe.

## Ground rules
- `cargo check -p k2so-daemon` before any commit; commit small and often with
  clear messages so cherry-pick order is reconstructible.
- No version bumps, no WHATS_NEW edits (release.sh owns those; Rosson's go).
- Integration = cherry-pick onto main; never merge the branch.
