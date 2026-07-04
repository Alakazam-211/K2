# Presence / Multiplayer Arc — Research + Build Plan

Date: 2026-07-04. Spec: Rosson (see memory `project_presence_multiplayer_arc.md`).
Research: 5 parallel Explore agents over timer, identity/roles, transport, nav, resize substrates.
Status: PLAN — nothing built yet.

## The five features

1. Timer → play/pause/stop **stopwatch** (delete countdown preset list).
2. **Presence roster** left of timer: ≤10 avatars + `+N`; click → modal (list all, kick, edit-grants).
3. **Viewer/claimer toggle** (top-right icon, per-window); new **viewer role** (view-only unless
   granted); grants are a toggle, **auto-revoked on disconnect**; role-colored chip borders.
4. **Workspace-nav presence**: 3 mini avatars + `+N` per workspace row, REPLACING git ± DiffStats.
5. **Pin-to-size** on pinned tabs: dropdown (80×24 / 100×30 / 120×36 / 160×48 / Match my window
   now); daemon holds fixed grid for everyone; small windows letterbox via existing scaleLayout.

---

## Substrate findings (condensed; file:line verified 2026-07-04)

### A. Timer — almost free

- Whole widget = `src/renderer/components/Timer/TimerButton.tsx` (151 lines).
  `startTimer()` in `stores/timer.ts:282` ALREADY starts a count-up timer (countdown is just an
  optional `targetDurationMs`); the running display already computes `displayMs = elapsed` when
  target is null (`TimerButton.tsx:94-99`). Pause/resume/stop cluster `:101-150` is stopwatch-ready.
- DELETE: `DURATION_PRESETS` + idle preset list (`TimerButton.tsx:4-13`, `:58-91`) → replace with a
  single play button calling `startTimer()`; countdown-expiry effect `:37-43`;
  `ExtendTimerDialog.tsx` + its App.tsx renders (`:233-235`, `:832-834`, `:892-894`) + store
  extend actions (`timer.ts:426-456`); `startWithDuration` (`:263-280`) + `targetDurationMs`.
  Settings: `timer.countdown*` entries in `Settings/sections/TimerSection.tsx`.
- KEEP: run/pause/stop UI (recolor from countdown red), `MemoDialog`, cross-window `sync:timer`
  broadcast (`useWindowSync.ts:60`), `time_entries` DB pipeline (`/cli/timer/create` etc.) —
  entries are duration-agnostic. **Zero daemon changes.** No sounds exist. Only test =
  `stores/timer.test.ts` (entries CRUD; presets untested — deletion safe).
- OPEN: `CountdownOverlay.tsx` (3-2-1 start animation + themes) is independent of presets —
  keep as optional flourish or delete with `timer-themes.ts`. Recommend: delete (countdown-themed,
  settings-simplification), Rosson may veto.

### B. Identity / roles / kick — primitives exist, liveness does not

- Users live in `~/.k2so/connect-users.json` (NOT drizzle DB): `crates/k2-core/src/connect_users.rs`
  — `ConnectUser{username, argon2 hash, disabled, role, token_epoch}` @130. `Role` @62:
  **Owner/Admin/Member only — NO viewer role anywhere.** Helpers: `can_manage_users` @103,
  `can_change_roles` @108, `can_act_on(actor,target)` @118.
- Sessions in `~/.k2so/connect-sessions.json`: `SessionRecord` @647 = token_digest + expiry +
  epoch. **No session id** (identified only by digest); `created_ip`/`user_agent` fields exist but
  are always None. One login = one record; N windows after one login share one token.
- **No kick route.** Core primitive `revoke_user_sessions(username)` @1081 (all-sessions kill) is
  unexposed — only reachable via disable/remove/set-password/set-role. `logout_session` @1093 is
  self-only (needs the raw token).
- **The daemon has NO "who is connected now" registry.** Liveness is implicit in per-socket WS
  loops that re-validate their token every 5s (`routes/http.rs:144` `token_still_valid`); nothing
  aggregates them. The tunnel (`tunnel_tls_listener.rs`) splices remote streams to loopback —
  remote is indistinguishable from local at the dispatcher except the `?token=` value.
- **Owner is not a session**: local windows auth with the daemon token → `actor_role` @162 maps it
  to `Role::Owner` with no user row and no session record. Roster must synthesize the owner entry.
- Auth-gate pattern to copy: `require_owner_or_admin` (`routes/http.rs:462`, async 403-on-fail) +
  in-handler `can_act_on` (mirror `handle_remove` in `connect_users_routes.rs:82-86`).
- Renderer users UI = `Settings/sections/K2ConnectSection.tsx` (`K2Role` @122 — add 'viewer';
  fetch plumbing `userGet/userPost` @213/@226; `useServerSupports('roles')` capability gating).

### C. Transport — additive layer on the session-events bus, NOT a new transport

- Spine: process-wide `tokio::sync::broadcast` in `crates/k2-daemon/src/session_events.rs`
  (`SENDER` @324, cap 256). `SessionEvent` tagged enum @47-322 — header says adding variants is
  wire-safe (clients ignore unknown `kind`). App-level events fan to ALL subscribers
  (`session_events_ws.rs:event_matches_workspace` @205 returns true for app-level).
- Every window already holds the app-level socket from boot: `subscribeToActiveState()`
  (`stores/session-events.ts:591`, opened App.tsx:293) → `/cli/sessions/events?path=&token=`.
  Reconnect = backoff 500ms→5s; on `hello` it re-fetches a snapshot (`GET /cli/projects/active`)
  — the delta+snapshot reconcile pattern presence must mirror.
- Per-workspace sockets already announce `?path=<workspace>` at upgrade
  (`session-events.ts:312`) — used only as a filter then discarded. **That param is the
  per-workspace presence signal, free.**
- Identity is on every upgrade (`?token=`) but only auth-checked, never resolved/stored.
  Precedent for resolving it: `authorize_send_message` (`http.rs:614`) → username from token.
  `subscriber_id` (AtomicU64, `session_events_ws.rs:96`) exists but is log-only.
- **Drop detection = TCP close only** (`read.next() → None`); no pings originate from either side.
  Fine for clean closes; a yanked network lingers until TCP timeout. Grants-revoke-on-disconnect
  needs faster: add a server-originated WS ping (registry entry reaped after N missed pongs).
- Kick latency note: revoking sessions alone leaves live sockets up to ~5s (their re-auth timer).
  The registry should hold a per-connection close/notify handle so kick tears sockets down now.

### D. Workspace nav — two JSX lines, one UX casualty

- Render sites: `Sidebar.tsx:353` (project row) + `:449` (worktree row), both via
  `AgentOrDiffStats` → `DiffStats` (`Sidebar.tsx:97-149`). IconRail shows counts only in the
  hover tooltip (`IconRail.tsx:42-58`).
- **CASUALTY: DiffStats doubles as the "AI Commit" button** (hover flips the badge to an
  AI-commit action, `Sidebar.tsx:140`). Replacing it removes that entry point — needs a decision.
- Pipeline (`useGitChanges` → `/cli/git/changes`, 30s poll) MUST STAY — ChangesPanel, WorktreeBar,
  dirty dots, branch labels consume it. Only the badge JSX goes. Dead code deletable:
  `AggregatedDiffStats` (183-211), `AheadBehind` (215-226), `workspacePaths` (602-605).
- Rows are tight (`py-1`–`py-2`): avatars ~14-16px. Join keys in scope at every row:
  `workspace.id` + resolved `worktreePath ?? project.path` (path matches the WS `?path=` signal —
  use PATH as the presence join key).
- **No user-avatar component exists.** Build `PresenceAvatar` forking `ProjectAvatar`'s
  initials-fallback (`Sidebar/ProjectAvatar.tsx:78,107-125`): circular, size prop, role-colored
  2px border. Role colors (proposal): owner amber/gold, admin purple, member blue, viewer gray.
  **V1 is initials-ONLY by decree (Rosson 2026-07-04)**: profile images arrive later from K2
  Connect Cloud accounts (the future identity source alongside Federation Groups; daemon-local
  user/pass stays forever). Keep an optional `imageUrl` prop in the component shape so Cloud
  avatars drop in without rework, but build nothing image-fetching now.
- No tests assert on DiffStats or row DOM — swap is test-silent.

### E. Resize / pin-to-size — one clamp point, three claim paths to suppress

- Arbitration state on `DaemonPtySession` (`crates/k2-core/src/terminal/daemon_pty.rs`):
  `active_subscriber` @445, `active_cols/rows` @488, `viewports` @498, `resize_gate` @504,
  `resize_generation` @520. Front door `request_resize` @1088 (debounce + same-dims skip).
  Detach re-elects most-recent survivor and resizes to THEIR dims (`detach_subscriber` @1015,
  `elect_on_detach` @319).
- Resize path: `TerminalPane.tsx sendResize` @2187 → `{action:'resize',cols,rows}` over
  `/cli/sessions/grid` (dispatcher @907, auth @912 = owner token OR stream_token) →
  `sessions_grid_ws.rs` Resize handler @883-927, acceptance gate @901
  (`active == 0 || active == subscriber_id`) → `request_resize`.
- **Messages carry no identity.** Auth class (owner vs stream-token) is known at accept only,
  never threaded into subscriber state. Input handler @833 writes unconditionally AND
  auto-steals the active claim + snap-resizes (@849-880) — viewer enforcement must gate both.
- **The juggling** (what pin kills): typing in any window steals the claim and reflows the shared
  PTY to that window's dims; back-and-forth on every focus/typing crossover.
- scaleLayout (`TerminalPane.tsx:2715`): passive branch @2757 letterboxes at
  `max(fit, 0.4)` when `!isActiveViewer` — the fit<1 path IS pin-to-size rendering, already
  built. Needed: a `pinnedSize` branch that computes fit against the pinned grid regardless of
  `isActiveViewer` (else the local claimer renders 1:1 and clips), and revisit
  `PASSIVE_SCALE_FLOOR = 0.4` for big pins in small windows.
- Persistence: `workspace_tab_sessions` (0045, daemon-owned, keyed project_id+pane_group_id) —
  add `pinned_cols/pinned_rows/pinned_set_by`. **Next migration = 0065** (0064_feedback is
  latest; 0057 gap is historical). Live: pinned atomics next to `active_cols/rows`; broadcast pin
  changes via the label-change channel pattern (@528-540).
- Dropdown mount: `PaneLayout/PaneTabBar.tsx` tab button ~@171 — no per-tab menu exists yet.
- "Match my window now" = freeze current `active_cols/active_rows` @488.

---

## Architecture decisions

1. **Presence registry lives in the daemon** (daemon-first), new module
   `crates/k2-daemon/src/presence.rs`: `Mutex<HashMap<conn_id, PresenceEntry>>` where
   `PresenceEntry = { conn_id, identity: Owner | ConnectUser{username, role}, kind:
   AppSocket | WorkspaceSocket{path} | GridViewer{session}, connected_at, close_handle }`.
   Registered/deregistered by the WS handlers (session_events_ws first; grid WS later for
   enforcement). "Connected" = holds the app-level events socket — every window (local or
   remote) opens one at boot, so this is exactly "windows open on this daemon".
2. **Roster event + snapshot**, mirroring ActiveChanged: new
   `SessionEvent::PresenceChanged { roster }` (app-level, whole-set, last-write-wins) +
   `GET /cli/presence/roster` fetched on `hello`. Roster aggregates per-user:
   `{ username|owner, role, window_count, workspaces: [paths], granted_edit }`.
3. **Viewer role** = new `Role::Viewer` below Member (`connect_users.rs` @62 + wire strings +
   renderer `K2Role`). Serde-default stays Member so existing rows are untouched. Capability-gate
   in renderer via `useServerSupports`.
4. **Edit grants are ephemeral in-memory state on the presence registry** (NOT persisted —
   Rosson decided grants die with the connection): `granted: HashSet<username>`; toggle route
   `POST /cli/presence/grant {username, granted}` gated `require_owner_or_admin`; auto-revoke
   when the user's LAST connection deregisters; `PresenceChanged` re-broadcast on every change.
5. **Kick = per-user in V1**: `POST /cli/presence/kick {username}` gated
   `require_owner_or_admin` + `can_act_on` (admin can't kick owner/admin peers per existing
   matrix). Implementation: `revoke_user_sessions(username)` + walk registry closing that user's
   sockets immediately (no 5s wait). Per-window kick deferred — needs session ids on
   `SessionRecord` (add the id field now while touching the struct, expose later).
6. **Claimer is per-window (mode), grants are per-user (permission)** — resolves the open
   question. The top-right toggle flips THIS window between viewer/claimer; whether claimer mode
   is USABLE depends on role ⊕ grant: owner/admin/member always may claim; viewer role only
   while granted. Enforced daemon-side in the grid WS (input + resize + SetActive gates), UI is
   advisory only.
7. **Presence join key for nav = workspace path** (prefix-match, same rule as
   `event_matches_workspace`), joined against each row's resolved `worktreePath ?? project.path`.
8. **Liveness ping**: server-originated WS ping on the app-level events socket (e.g. every 10s,
   reap after 2 misses) so disconnect-revokes-grants fires within ~20s on hard drops, instant on
   clean closes.
9. **Owner appears in the roster** as a synthesized always-on entry per local window (owner
   windows hold the same sockets; identity = Owner, display "Owner"/machine user).

## Build order (slices; subagent-worktree → cherry-pick per convention)

- **S0 — Stopwatch** (small, independent, ships alone): delete presets/extend/target; single
  play button; recolor running UI; settings cleanup. Renderer-only.
- **S1 — Presence substrate (daemon)**: identity resolution at events-WS upgrade; registry +
  deregister-on-teardown; server ping; `PresenceChanged` + `GET /cli/presence/roster`
  (owner synthesized). Record `?path=` from per-workspace sockets. Headless-testable:
  open 2 sockets w/ different tokens, curl roster, drop one, assert broadcast. POST-only guards
  on mutating routes.
- **S2 — Roster UI**: `PresenceAvatar` (initials, role-colored border); top-bar roster (≤10 +
  `+N`) in TopBar RIGHT group before the Run button (+ FocusLayout twin); presence modal
  (read-only list first); renderer presence store (snapshot-on-hello + event dispatch).
- **S3 — Kick**: route + registry socket-close + session-id field on `SessionRecord`; modal kick
  buttons (owner/admin, `can_act_on` mirrored client-side for UI affordance).
- **S4 — Viewer role + grants**: `Role::Viewer` end-to-end (core, wire, K2ConnectSection,
  capability gate); grant toggle route + auto-revoke-on-disconnect + modal toggles.
- **S5 — Claimer enforcement (grid WS)**: thread auth class/identity into grid subscriber state;
  gate Input (@833) / Resize (@901) / SetActive; suppress claim-steal for non-claimers;
  top-right per-window toggle UI.
- **S6 — Nav presence**: replace DiffStats at Sidebar.tsx:353/:449 with avatar cluster (join by
  path); IconRail tooltip swap; delete dead aggregate components. (Blocked on AI-Commit
  relocation decision.)
- **S7 — Pin-to-size**: migration 0065 (pinned cols/rows/set_by on workspace_tab_sessions);
  pinned state on DaemonPtySession + clamp at top of `request_resize` (covers grid-WS, claim,
  detach-promote paths); pin broadcast (label-channel pattern); PaneTabBar dropdown + routes;
  scaleLayout `pinnedSize` branch + floor revisit.

Dependencies: S1 → S2 → S3/S4 → S5; S6 needs S1(+S2's avatar); S7 independent of S2-S6 (can run
parallel); S0 anytime. Suggested waves: [S0, S1] → [S2, S7-daemon] → [S3, S4] → [S5, S6, S7-UI].

## Open questions — ALL DECIDED by Rosson 2026-07-04 (see PRD §10)

1. **AI Commit button**: DROPPED entirely (not relocated).
2. **CountdownOverlay (3-2-1 themes)**: DELETE with the countdown + settings cleanup.
3. **Role colors**: owner=amber-gold, admin=purple, member=blue, viewer=gray.
4. **Viewer-mode semantics**: no-input AND no-resize; claimer mode grants both.

PRD: `.k2/prds/prd-presence-multiplayer-v1.md` — the build SSOT. This note remains the
substrate/file:line reference.

## Testing strategy

- Daemon: integration tests per slice (`*_integration` naming!) — roster over 2 fake tokens,
  kick closes sockets, grant lifecycle incl. disconnect-revoke, pin clamp across all 4 resize
  paths (grid resize / input-claim / SetActive / detach-promote), viewer input dropped.
- Renderer: vitest for presence store reducer + roster aggregation; no DOM tests needed for nav
  swap (none exist).
- Live e2e: two windows + one K2 Connect remote login against the dev daemon; the classroom
  scenario (pin 100×30, viewer joins small window, grant toggle, kick).
- Baselines: tsc 67 pre-existing; vitest 931; k2-core lib 1184; flaky-solo daemon tests
  (cell_uds, connect_users logout, v1_sandboxes::policy) pass solo.
