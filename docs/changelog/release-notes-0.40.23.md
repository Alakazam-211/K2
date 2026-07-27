# K2 0.40.23 — Heartbeats you can trust

## Heartbeat reliability overhaul

- **Misses always catch up.** The croner evaluator is now the single
  due-authority ("due iff next occurrence after last-fired ≤ now"); the legacy
  calendar-position gate — which silently dropped any miss that crossed a
  day/week/month boundary — is gone. One coalesced catch-up fire for the most
  recent missed occurrence, audited `fired_catchup` with the originally
  scheduled time.
- **Boot overdue scan + wall-clock-jump detection.** A daemon restart or a
  laptop waking from sleep evaluates due schedules immediately — no waiting for
  the next external tick. `last_tick_at` is persisted so gaps are measurable.
- **Per-heartbeat firing windows.** Optional start/end time-of-day range per
  heartbeat; out-of-window occurrences neither fire nor count as missed, and a
  due catch-up holds until the window opens.
- **Manual-fire latch removed.** Hand-launching an agent no longer consumes the
  day's scheduled fire.
- **Failure policy.** Exponential backoff (1/2/4/8 min, capped) and
  auto-disable after five consecutive failures, with a visible badge and audit
  row; success or manual re-enable resets. The spawn-timeout lease leak that
  could wedge a heartbeat until daemon restart is fixed (the deferred-to
  watchdog now exists).
- **Transport self-heal.** The heartbeat LaunchAgent is verified and
  reinstalled automatically if missing, with a "transport down since…" banner
  in Wake Scheduler settings (it was previously possible for the scheduler to
  be silently absent indefinitely).
- **Audit hygiene.** No more per-tick `not_due` rows (previously ~1,440 rows
  per heartbeat per day); the 90-day pruner actually runs; monthly
  `days_of_month` schedules no longer degrade to day 1 in cron translation.

## Workspace switching

- **Poisoned layouts fully self-heal.** Saved layouts carrying large numbers of
  leaked empty terminal tabs (debris from a pre-0.40.22 client bug) are pruned
  to their real tabs on load and save — renamed, pinned, heartbeat-, sandbox-,
  and resumable tabs are always preserved.
- **Lazy shell spawn for restored tabs.** Entering a workspace no longer spawns
  a shell for every hidden restored terminal tab; never-attached, session-less
  tabs spawn on first view. Warm paths (active tab, pinned chat, resumable
  sessions, heartbeat-surfaced tabs, command-carrying spawns) stay eager.

## Known / deferred

- Occasional screen tearing during very fast scrolling remains under
  investigation (compositor artifact; the experimental WebGL painter avoids
  it).
- Daemon-side activity detection and per-peer federation trust management are
  tracked for a later release.
