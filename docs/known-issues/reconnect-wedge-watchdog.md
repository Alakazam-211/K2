# Reconnect-wedge / webview watchdog — solved for now, revisit later

**Status:** Solved for now (P1 harm-reduction shipped / shippable with 0.40.58-class work)  
**Revisit:** Yes — do not treat as a full fix of Ops BUG 2  
**Related:** Ops forensics 2026-07-22 (`HANDOFF-client-reaper-forensics-for-grok.md`); 0.40.48 reconnect arc; 0.40.57 active-reaper P0

## What we call “solved”

Two client-side amplifiers from the control-plane-blip incident are bounded:

1. **Grid WS park (PR-B)** — visible terminal panes no longer schedule timed grid reconnects while remote `recovery.kind !== 'connected'`; they park on `onceRecovered` (same bus as session-events).
2. **Watchdog anti-thrash (PR-C)** — native webview reload no longer resets to `attempt 1/3` on a brief post-reload heartbeat. Episode needs a sustained healthy streak; reload budget is real; cooldown 30s → 60s → 120s; then GiveUp.

These address the field log signature (`native reload attempted (attempt 1/3) -> ok` ×N) and reconnect-herd pressure. They are **seatbelts**, not a full soft-reconnect cure.

## What is *not* solved (revisit when it recurs)

Independent design reviews (blind, 2026-07-22) agreed:

| Still open | Why it matters |
|------------|----------------|
| **Root of heartbeat silence** | Ops proved thrash, not *why* JS went intermittent (main-thread stall vs content-process vs dual reload paths). |
| **ConnectionGate / “NSI down” copy** | User-facing “refused to connect” while remote stayed healthy may be recovery-UI semantics, not a dead webview. |
| **Control-plane blip vs host-down** | 0.40.48 targeted host reboot / poisoned pool; this failure mode is client loss of `connect.k2.dev` with a healthy host. |
| **Park gate completeness** | Grid park keys off `recovery.kind`; if that stays `connected` during a control-plane-only blip, park never arms. |
| **Watchdog as reconnect tool** | Reloading the whole webview mid-storm can wipe client state; anti-thrash only rate-limits that self-harm. |

If the app still “looks down until relaunch” after PR-B/PR-C, **do not just tighten the watchdog further**. Instrument `recovery.kind`, grid park vs timed retry, heartbeat gaps, and ConnectionGate state across the window.

## Framing for release notes

- **Do** say: no more unbounded native-reload thrash; terminals don’t hammer the edge during remote recovery.
- **Do not** say: “reconnect-wedge fully fixed” or “control-plane blips are transparent.”
- Keep **0.40.57** as the story for live sessions surviving blips (reaper); keep this as **client recoverability / anti-thrash**.

## When to reopen

- Same Ops signature returns **with** advancing attempt counters (different bug).
- GiveUp fires too early on long soft recovery (false permanent death).
- True black-screen recovery feels too slow (cooldown latency).
- Field dump shows thrash **without** recovery.kind leaving `connected` (park never engaged).

## Code anchors

- `src-tauri/src/webview_watchdog.rs` — pure episode / cooldown FSM
- `src-tauri/src/lib.rs` — host loop + field log
- `src/renderer/kessel-term/activeViewer.ts` — `shouldScheduleGridReconnect`
- `src/renderer/kessel-term/TerminalPane.tsx` — grid `onclose` park on `onceRecovered`
