---
title: Heartbeat misfire study + polish plan
status: study — no code changes
created: 2026-07-02
author: research agent (Fable 5), commissioned by Rosson
question: "Why do cron jobs, if missed (laptop dead), never reset or fire again later? Heartbeats are finicky — plan to polish it off."
---

# Heartbeat misfire study + polish plan

Short answer to the owner's question: **missed windows are not dropped by the
timer — they are dropped by the eligibility gate.** The 0.38.2 croner rework
(`heartbeats/cron.rs`) already made `is_due` catch-up-friendly ("if now is at
or past the next occurrence after last_fired, fire"), but a *second, older*
evaluator — `should_project_fire` in `crates/k2-core/src/scheduler.rs` — still
runs FIRST on every tick and re-evaluates the schedule against *today's
calendar position* (minute-of-day, weekday, day-of-month) instead of against
`last_fired`. Any miss that isn't recovered on the *same calendar day* (or
same weekday / day-of-month) is silently skipped as `skipped_schedule /
"window not open"` until the next scheduled slot. For a weekly Monday report
missed while the laptop was dead, that is a full week of silence.

---

## Part 1 — System map (file:line)

### 1.1 What drives the tick

There is **no tokio interval inside the daemon** for heartbeats. The only
autonomous driver is the OS scheduler:

- launchd LaunchAgent `dev.k2.heartbeat` (macOS) / crontab entry (Linux),
  installed by `crates/k2-core/src/heartbeats/install.rs`
  (`ensure_cron_installed`, called on first `heartbeat add`;
  `install_heartbeat_launchd` at install.rs:376). `StartInterval` default 60s
  (install.rs:29), `RunAtLoad=false` (install.rs:206).
- The plist runs `~/.k2/heartbeat.sh` (template at install.rs:87-169). The
  script: reads `~/.k2/heartbeat.port` + `heartbeat.token`, health-checks the
  daemon, asks `GET /cli/heartbeat/active-projects` (served by
  `crates/k2-daemon/src/triage.rs:77` — DISTINCT project paths with enabled
  non-archived `workspace_heartbeats` rows), then hits
  `GET /cli/scheduler-tick?project=<path>` per project.
- If the daemon is down, the script `exit 0`s silently (install.rs:113-124) —
  nothing anywhere records that a tick minute passed unserved.
- One manual driver exists: Settings → ProjectsSection "tick now"
  (`src/renderer/components/Settings/sections/ProjectsSection.tsx:2001`).

### 1.2 What a tick does

`/cli/scheduler-tick` → `handle_scheduler_fire`
(`crates/k2-daemon/src/triage.rs:111`). Two lanes, in order:

**Lane A — inbox/project-level wake** (`k2so_agents_scheduler_tick`,
`crates/k2-core/src/workspace/scheduler.rs:356`): workspace-state gate
(heartbeat=0 halts, :401), project-level `heartbeat_mode`/`heartbeat_schedule`
gate (:416-440), then wakes agents whose inboxes have items. Fires via
`wake_headless::spawn_wake_headless` (triage.rs:146).

**Lane B — the multi-heartbeat rows** (`k2so_agents_heartbeat_tick`,
`crates/k2-core/src/heartbeats/mod.rs:318`): iterates enabled
`workspace_heartbeats` rows and for each runs:

1. `should_project_fire(hb.frequency, hb.spec_json, hb.last_fired)`
   (mod.rs:338, impl `crates/k2-core/src/scheduler.rs:35`) — the LEGACY
   calendar-position gate. Fail → audit `skipped_schedule "window not open"`
   and `continue` (mod.rs:343-357).
2. `cron::is_due(&hb)` (mod.rs:369, impl
   `crates/k2-core/src/heartbeats/cron.rs:45`) — the 0.38.2 croner check:
   never-fired → due now; hourly → `now >= last_fired + every_seconds`;
   daily/weekly/monthly/yearly → `now >= croner.next_occurrence(last_fired)`.
   Fail → audit `not_due`.
3. WAKEUP.md existence — missing → **auto-disable** the row + audit
   `wakeup_file_missing` (mod.rs:386-408).

Survivors become `HeartbeatFireCandidate`s, fanned out bounded-concurrent
(6 parallel, 30s deadline each) through `smart_launch`
(`crates/k2-daemon/src/triage.rs:201`, `crates/k2-daemon/src/heartbeat_launch.rs:62`).

### 1.3 What fires and what state is written

`smart_launch` (heartbeat_launch.rs:62):

- CAS-acquires the row's in-flight lease `in_flight_started_at`
  (`AgentHeartbeat::try_acquire_heartbeat`,
  `crates/k2-core/src/db/schema.rs:1897`, BEGIN IMMEDIATE; `forbid` policy
  refuses when already set).
- Picks a branch via the pure planner `plan_launch_decision`
  (heartbeat_launch.rs:297): fresh fire (spawn
  `claude --print --append-system-prompt <WAKEUP body>` PTY) / inject into a
  live PTY / `--resume` + fire. `use_workspace_session` rows route through
  `workspace_msg::deliver_live` instead (heartbeat_launch.rs:111).
- **Success**: `stamp_fired_and_release` — one UPDATE sets
  `last_fired = now (UTC RFC3339)` AND clears the lease (schema.rs:1966).
  Plus `last_session_id` / `active_terminal_id` stamps, audit row `fired`.
- **Failure**: `release_lease` only — `last_fired` untouched, audit `error`
  (e.g. heartbeat_launch.rs:448-454). The row stays eligible and will be
  retried on the next tick. (Good honesty; but see §1.6 — no backoff.)

Audit trail: every decision writes a `heartbeat_fires` row
(`HeartbeatFire::insert_with_schedule`), surfaced by `k2 heartbeat status`,
the workspace History panel (`k2so_heartbeat_fires_list`, mod.rs:499) and the
system-wide WakeSchedulerSection (mod.rs:518).

### 1.4 THE misfire drop point (exact)

The croner half would recover any miss; the legacy gate in front of it drops
the recovery. The drop is:

> **`crates/k2-core/src/heartbeats/mod.rs:343`** — `if !eligible { …
> "skipped_schedule", "window not open" … continue; }` — where `eligible`
> came from `should_project_fire` (mod.rs:338), whose calendar-position
> checks live in `crates/k2-core/src/scheduler.rs`:
>
> - **:117** `if now_mins < schedule_mins { return false; }` — a scheduled
>   fire missed yesterday stays dropped until *today's* wall-clock time-of-day
>   passes again; combined with the checks below it can never fire "late
>   across a day boundary."
> - **:150** (weekly) `day_arr.iter().any(|d| d.as_str() == Some(weekday))` —
>   `weekday` is **today's** weekday. Laptop dead through Monday → machine
>   wakes Tuesday → `"tue" ∉ ["mon"]` → the entire week's fire is dropped.
> - **:154-170** (monthly `days_of_month` / ordinal) and **:172-193**
>   (yearly months) — same shape: they test *now*'s calendar slot, never
>   "was an occurrence missed since last_fired."

Concrete traces (all verified against the code, weekly/daily verified live in
§1.8):

| Scenario | Legacy gate | croner `is_due` | Net result |
|---|---|---|---|
| Daily 09:00, slept 08:50–10:00 same day | pass (now ≥ 09:00) | due | **fires late — OK** |
| Daily 09:00, dead until next day 08:30 | **:117 drops** | (due) | skipped; fires next day 09:00 — yesterday's occurrence silently lost |
| Weekly Mon 09:00, dead Monday, wake Tuesday | **:150 drops** | (due) | skipped until NEXT Monday |
| Monthly 15th, dead on the 15th | **:154 drops** | (due) | skipped until next month |
| Hourly every N s (any pause) | pass (window ⊆ 00:00-23:59) | due | fires on first tick back — OK |

So the owner's observation is precisely right for weekly/monthly and for any
daily miss that crosses midnight; hourly heartbeats do recover (that was the
0.38.2 fix — its module doc "Long pauses recover automatically" is only true
for the frequencies the legacy gate doesn't re-drop).

Also note `scheduler.rs:122-130`: an unconditional **once-per-day latch** —
any `last_fired` today blocks all further scheduled fires today (for every
frequency, not just daily). A manual "Launch now" stamps `last_fired`
(smart_launch stamps regardless of trigger), so a morning manual fire
*consumes* that day's scheduled fire.

### 1.5 Sleep vs dead — both paths

- **(a) Daemon/machine restarts:** launchd loads the agent at login,
  `RunAtLoad=false`, first tick ≤ StartInterval (60s) later. The daemon does
  **no boot-time overdue scan** — boot only sweeps stale leases
  (`main.rs:296`, `sweep_stale_leases(300)`) and stale `active_terminal_id`s.
  Recovery is entirely delegated to "the next tick will catch it," which
  works only where the legacy gate lets it (see table).
- **(b) Slept through the window:** launchd `StartInterval` jobs that miss
  fires during sleep run **once** shortly after wake (launchd coalescing), so
  a wake tick does happen — no tokio-timer pause issue exists because there
  is no in-daemon timer. Again the eligibility gates decide, so same-day
  wake ⇒ daily recovers; cross-midnight / wrong-weekday wake ⇒ dropped.
- There is **no wall-clock-jump detection** anywhere, and no persisted
  record of "ticks that should have happened" — a user cannot distinguish
  "schedule evaluated and skipped" from "no tick ever arrived" (daemon down,
  plist unloaded, script exited early). If the plist was never installed or
  was removed (`apply_wake_scheduler("off")`, install.rs:609), enabled
  heartbeats sit dark forever with `enabled=yes`.

### 1.6 The "finicky" audit — everything fragile found

1. **Dual evaluators, single source of misfire** — `should_project_fire`
   (legacy, calendar-position) AND `cron::is_due` (croner, last_fired-relative)
   must BOTH pass (mod.rs:338 + :369). Two grammars for the same spec_json
   (scheduler.rs:111-196 vs cron.rs:111-151) must be kept semantically in
   sync by hand; the misfire bug is exactly a disagreement between them.
2. **Wedge: in-flight lease leaked on spawn timeout.** The 30s
   `tokio::time::timeout` in `run_candidates_bounded`
   (`crates/k2-daemon/src/triage.rs:235-246`) abandons the spawn WITHOUT
   releasing `in_flight_started_at` — the comment says "cleared by the
   boot-time sweep next time we restart, or by an explicit release in P5.5's
   watchdog," and that watchdog was never built (the shipped
   `daemon/src/watchdog.rs` is a session-output watchdog, unrelated). Under
   `concurrency_policy='forbid'` (default) the heartbeat is **wedged until
   the daemon restarts** (boot sweep, 300s threshold, main.rs:296). A hung
   PTY allocate on a long-lived daemon = heartbeat dark indefinitely,
   audit shows only `skipped_locked`.
3. **Unparseable spec = enabled-but-dark forever.** `is_due` returns false
   for unparseable/unknown specs by design (cron.rs:41-44) — "stuck but
   loud" only in the audit log; the Settings row still reads enabled/healthy.
   Nothing ever flags "this schedule can never fire."
4. **No retry backoff on fire errors.** Failure honestly leaves `last_fired`
   unset (heartbeat_launch.rs:449, schema.rs:1951) — correct — but the row
   is then retried every 60s tick forever (spawn error, WAKEUP body empty
   pre-0.38.12-style loops). One `error` audit row per minute; the
   auto-disable remedy exists only for the missing/empty-WAKEUP case
   (heartbeat_launch.rs:779).
5. **Lane A advances its stamp before spawning.** The project-level
   scheduled mode stamps `heartbeat_last_fire` BEFORE any spawn happens
   (`workspace/scheduler.rs:433-438`); if every subsequent spawn fails, the
   window is consumed anyway. (Lane B does this correctly.)
6. **Once-per-day latch interacts with manual fires** (scheduler.rs:122-130,
   see §1.4) — manual Launch consumes the day's scheduled fire; editing the
   time later the same day also can't fire until tomorrow.
7. **`heartbeat_fires` grows unboundedly.** Every tick writes a `not_due` /
   `skipped_schedule` row per enabled heartbeat (mod.rs:344, :370) — 1,440
   rows/heartbeat/day at the 60s default. `HeartbeatFire::prune_before`
   exists (schema.rs:2566) but has **zero callers**. Real fires drown in
   no-op rows; DB bloats.
8. **Dead schema.** `starting_deadline_secs` is vestigial post-0.38.2 (only
   the struct/README mention it); `concurrency_policy='replace'` is
   unimplemented (behaves as `allow`, schema.rs:1891-1892);
   `k2so_heartbeat_tick`'s doc still references `stamp_heartbeat_fired`
   being the caller's job while smart_launch owns stamping.
9. **Timezone/DST.** `last_fired` is stamped UTC (schema.rs:1971), compared
   in Local (cron.rs:50) — sound. But croner occurrences are computed in
   Local: a `time` inside the spring-forward gap (e.g. 02:30) or the
   fall-back repeat hour is ambiguous; `find_next_occurrence(...).ok()?`
   silently degrades to "not due" (cron.rs:92-93). Untested territory. The
   legacy `daily interval>1` path uses `day_of_year % interval`
   (scheduler.rs:139) — resets/aliases at year boundaries and ignores
   last_fired entirely.
10. **First fire is immediate, not scheduled.** `last_fired=None` → due now
    (cron.rs:51-54); a daily-09:00 heartbeat created at 15:00 fires at
    15:00 today (gate passes since now ≥ 09:00). Surprising; also seeds
    `last_fired` at an off-schedule time.
11. **Sessions that no longer exist** — well defended, for the record: dead
    PTY / reaped-session cases route to resume-or-fresh via the planner's
    liveness gates (heartbeat_launch.rs:297-335), ghost JSONL self-heals
    (clear_session_id, schema.rs:1743), boot sweeps null stale
    `active_terminal_id` (main.rs:~305). The 0.39.x Active-reaping races
    appear closed (dismiss-reap invariant is unit-tested,
    heartbeat_launch.rs:849). No new races found between the scheduler and
    Active reaping; `is_agent_locked`/user-session guards cover Lane A
    (workspace/scheduler.rs:514-543).
12. **Silent tick-transport failures.** heartbeat.sh exits 0 on: no port
    file, bad port, daemon unhealthy, no token (install.rs:113-134). Only
    some failure modes append to `~/.k2/heartbeat.log`. Nothing in the DB
    records tick arrival, so "why didn't it run?" has no answer when the
    transport (plist/script/daemon-down) is the cause — the single biggest
    observability hole.

### 1.7 Prior art in the house

- **0.38.2 croner rework** (cron.rs module doc) — already chose
  fire-on-late-catch-up over skip-because-late after the "11 triage
  heartbeats dark 22+ days" incident. The polish plan below finishes what it
  started by retiring the legacy gate.
- **PRDs**: `.k2/prds/multi-schedule-heartbeat.md` (folder-per-heartbeat
  data model, audit-trail goals — silent on misfire policy) and
  `.k2/prds/heartbeat-active-session-tracking.md` (active_terminal_id
  lifecycle; restart reconciliation for *sessions*, not schedules). Neither
  addresses missed windows — confirmed gap, not a regression.
- **OpenClaw patterns** (memory note `reference_openclaw_patterns.md`; no
  in-repo study file): *adaptive heartbeat* (backoff on consecutive no-ops —
  the deprecated `AgentHeartbeatConfig.auto_backoff`/`consecutive_no_ops`
  fields in workspace/scheduler.rs:118-162 were K2's copy of this, retired
  0.39.0d), *wake priority queue*, *coalescing* (many missed occurrences →
  ONE catch-up fire), and *active hours* (don't fire stale work at 3am; K2
  retains `is_within_active_hours` at workspace/scheduler.rs:251 and the
  hourly start/end window at scheduler.rs:79-95). Coalescing + active-hours
  are directly applicable to catch-up policy.
- **Quartz/cron misfire vocabulary** (for the decision): *fire-once-now*
  (one catch-up fire on recovery, with a max-age grace — Quartz
  `MISFIRE_INSTRUCTION_FIRE_ONCE_NOW`), *fire-all-missed* (replay every
  missed occurrence — almost never right for agent wakes), *skip-to-next*
  (today's de-facto K2 behavior). River/Oban-style persisted attempts +
  boot sweep is already half-adopted (lease sweep at main.rs:296).

### 1.8 Empirical check (scratch-HOME daemon)

Method: `cargo build -p k2-daemon` in this worktree; daemon run with
`HOME=<scratch>` (isolated `~/.k2`, own DB, ephemeral port) and a fake
`claude` shim first in PATH (echo+exit 0) so fires are cheap and real.
Project + heartbeat rows inserted via sqlite3 directly (deliberately NOT via
the add route — `ensure_cron_installed` would have run `launchctl bootout`
against the production `dev.k2.heartbeat` label). Ticks driven by curl
exactly as heartbeat.sh does. Production daemon untouched.

Six rows exercised (all times local, 2026-07-02, a Thursday):

| # | Setup | Expected from code | Observed |
|---|---|---|---|
| hb1 `ontime-catchup` | daily 17:04, last_fired yesterday 17:04, tick at 17:07 | fire late same-day | **fired** (`fired / fresh fire`), last_fired stamped 23:07:07Z |
| hb2 `missed-crossday` | daily 23:50, last_fired 3 days ago → June 30 + July 1 occurrences missed; tick 17:07 (< 23:50) | dropped at scheduler.rs:117 | **`skipped_schedule / "window not open"`** — misses silently discarded |
| hb3 `missed-weekly` | weekly days=["wed"] 09:00, last_fired 8 days ago → yesterday(Wed) missed; tick Thursday | dropped at scheduler.rs:150 | **`skipped_schedule / "window not open"`** — whole week lost |
| hb4 `hourly-catchup` | hourly 3600s, last_fired 22 days ago | fire on first tick | **fired** — the 0.38.2 recovery works for hourly |
| hb5 `kill-restart` | daily at now+1min, last_fired yesterday same time; **daemon killed at 17:07:35 before the 17:08 window; restarted 17:10** | fire late (same-day) | **fired at 17:10:06** — 2min late, no wedge. Same-day misses recover on restart |
| hb6 `fail-honesty` | due heartbeat, daemon restarted with no `claude` on PATH; ticked twice | error, no stamp, retry | **two `error / "spawn failed: … No such file or directory"` rows**, `last_fired` still NULL, lease released — honest, but blind re-attempt on every tick (no backoff) |

Tick responses: tick#1 → `{"count":2,"heartbeats":["hourly-catchup","ontime-catchup"]}`;
restart tick → `{"count":1,"heartbeats":["kill-restart"]}`; failure ticks →
`{"count":0}` with the error rows only in `heartbeat_fires`.

**Verdict:** the daemon neither wedges nor fires-all-missed. It fires late
*within the same calendar slot* (same day for daily, same weekday for
weekly), and **silently skips** anything that crossed a day/week/month
boundary — audit shows only the misleading `skipped_schedule "window not
open"`, indistinguishable from a schedule that simply isn't due yet.

**Bonus production finding (this machine, read-only):** the
`dev.k2.heartbeat` LaunchAgent is NOT loaded and no plist exists in
`~/Library/LaunchAgents/` (only `dev.k2.daemon` + `dev.k2.claude-auth`);
`~/.k2/heartbeat.log`'s last tick is **2026-06-10 17:28** — the production
tick driver has been dead for ~3 weeks while `heartbeat.sh` (current
template) and `heartbeat.port` sit ready. Every enabled heartbeat on this
box has been dark the whole time with zero user-visible signal — a live
instance of fragility #12 and very plausibly the root of the "finicky"
feeling. (Not repaired — study only; re-apply Settings → Wake Scheduler or
run any `heartbeat add` to reinstall.)

---

## Part 2 — Recommended misfire policy (the decision)

**Default: catch-up-once with a grace window, evaluated from persisted
`last_fired`.** Concretely: a schedule is *due* iff
`next_occurrence(after last_fired) <= now` — croner semantics, no calendar-
position gate. If `now - next_occurrence > catch_up_window` (proposed
default **24h**, per-schedule override later), record the occurrence as
`missed` in the audit log and advance to the next occurrence instead of
firing. Multiple missed occurrences always coalesce to at most ONE catch-up
fire (OpenClaw coalescing). Skip-to-next remains what happens *after* the
grace expires — but now it's recorded as `missed`, never silently.

Why not the alternatives: *fire-all-missed* would replay N wake prompts into
an agent session (token burn, duplicate reports); *pure skip-to-next* is the
current complaint. Catch-up-once matches user intent for agent wakes ("run my
Monday report when I'm back Tuesday, not never, and only once").

## Part 3 — The polish plan (4 PR-sized slices)

### Slice 1 — one evaluator, catch-up semantics (the core fix)
- Retire `should_project_fire` as a gate for Lane B: delete the call at
  `heartbeats/mod.rs:338-357`; make `cron::is_due` (extended) the single
  authority. Port the two things the legacy gate does that croner doesn't:
  the hourly `start`/`end` active window, and (optionally) the once-per-day
  latch — reimplemented as "next occurrence after last_fired" naturally
  prevents double-fires, so the latch can go.
- Extend `next_fire_time_after` to full spec coverage (days_of_month arrays,
  ordinal weekdays, yearly months — croner expressions already support all
  of these; today only the simple shapes are translated,
  cron.rs:111-151).
- Add the catch-up window: `is_due` returns an enum
  `{Due, DueCatchUp{missed_at}, MissedExpired{missed_at}, NotYet{next}}`;
  `MissedExpired` writes a `missed` audit row and stamps
  `last_fired = missed occurrence` so the next computation starts clean.
- Keep Lane A (project-level/inbox) on the old gate for now — it's
  event-driven (inbox items), not schedule-driven; but move its
  `heartbeat_last_fire` stamp to after-spawn-success
  (workspace/scheduler.rs:433).
- Tests: the five trace rows in §1.4 as table tests with injected `now`.

### Slice 2 — wake/restart honesty (transport + wedge)
- **Boot-time overdue scan**: at daemon boot (next to main.rs:296), run the
  Slice-1 evaluator over all enabled rows and fire/`missed`-stamp overdue
  ones — recovery no longer depends on the plist being alive.
- **Tick heartbeat**: persist `last_tick_at` (per daemon, one row) on every
  `/cli/scheduler-tick`; a wall-clock jump since the previous tick
  (> 3× interval) is logged as a `tick_gap` audit row — makes sleep gaps
  and daemon downtime visible and doubles as drift detection. Surface a
  Settings warning when `now - last_tick_at` is large while heartbeats are
  enabled ("scheduler is not ticking — reinstall wake scheduler").
- **Fix the lease wedge**: on the timeout path in triage.rs:235-246, spawn a
  deferred release (or run `sweep_stale_leases` at the top of every tick,
  threshold `max(active_deadline_secs)` — one UPDATE, cheap). Boot sweep
  stays as belt-and-suspenders.

### Slice 3 — failure honesty + hygiene
- Retry with backoff for `error` outcomes: persist `consecutive_failures`;
  retry on next tick ×3, then back off exponentially (cap: 1h), and after N
  (e.g. 10) auto-disable WITH a distinct `auto_disabled_failing` audit
  decision + UI badge (mirror the missing-WAKEUP auto-disable UX).
- Flag never-can-fire rows: when the evaluator returns "unparseable spec,"
  set a `schedule_invalid` surfaced state instead of silent `not_due` rows.
- Stop writing per-tick `not_due`/`skipped_schedule` audit rows (keep
  decisions that carry information: fired/missed/error/skipped_locked/
  auto_disabled); wire `HeartbeatFire::prune_before` into boot (e.g. 90-day
  retention).
- Drop/repurpose dead schema: `starting_deadline_secs`; either implement or
  remove `concurrency_policy='replace'`.

### Slice 4 — observability where users configure heartbeats
- Per-schedule strip in Settings → Heartbeats + sidebar: **last fired /
  last outcome / missed count / next fire** (next fire = Slice-1 evaluator's
  `NotYet{next}`, already computable via `next_fire_time_after`).
- `k2 heartbeat status <name>` gains `next: <ts>` and shows `missed` rows.
- The `missed`/`tick_gap`/`auto_disabled_failing` decisions from Slices 1-3
  are what make "why didn't it run?" answerable end-to-end: transport gap →
  `tick_gap`; schedule miss → `missed`; fire failure → `error`+backoff trail;
  operator action → `auto_disabled*`.

Sequencing note: Slice 1 alone answers the owner's complaint; Slice 2 makes
it robust to the dead-laptop case specifically; 3-4 are the "polish it off."

## Owner decisions needed

1. **Catch-up window length** — proposed default 24h. (A weekly report fired
   ≤24h late is useful; 6 days late is noise.) Per-schedule override now or
   later?
2. **Catch-up timing vs active hours** — fire stale heartbeats immediately on
   wake/boot (possibly 3am), or hold until the workspace's active-hours
   window (OpenClaw pattern; `is_within_active_hours` already exists)?
   Proposal: immediate for v1, active-hours gating as the per-schedule
   override that reuses the hourly `start`/`end` fields.
3. **Once-per-day latch** — keep "manual fire consumes today's scheduled
   fire," or let the scheduled fire still run after a manual launch?
   (Croner semantics naturally allow the latter; current behavior is the
   former.)
4. **Auto-disable threshold for failing fires** (Slice 3) — after how many
   consecutive errors, and should auto-disable notify (toast/inbox item) or
   just badge in Settings?
5. **First-fire semantics** — keep fire-immediately-on-create, or wait for
   the first scheduled occurrence (with a "Fire now" button covering the
   old behavior)?
6. **Audit retention** — 90 days? And confirm dropping per-tick `not_due`
   rows (loses the "prove the evaluator looked at it" trail in exchange for
   a readable history + bounded DB).
7. **Lane A's future** — project-level `heartbeat_mode`/`heartbeat_schedule`
   duplicates the per-row system; fold it into a `workspace_heartbeats` row
   or keep as-is? (Out of scope for slices 1-4 except the stamp-order fix.)
