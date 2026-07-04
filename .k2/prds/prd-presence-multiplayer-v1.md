# PRD — Presence & Multiplayer V1

Status: APPROVED FOR BUILD (queued after the Feedback arc).
Date: 2026-07-04. Owner: Rosson. Author: Claude (research + synthesis).
Research SSOT: `.k2/notes/presence-multiplayer-research.md` (file:line substrate map, verified 2026-07-04).
Spec lineage: Rosson verbal spec 2026-07-04 + 4 product decisions recorded 2026-07-04 (§10).

## 1. Problem

A K2 daemon can already be used by several people at once (local owner windows + K2 Connect
remote logins), but the product is blind to it: nobody can see who else is connected, viewers
silently fight over terminal size (typing in any window reflows the shared PTY to that window's
dims), there is no way to let someone watch without touching, and no way to remove someone
without disabling their account. The top-bar timer's countdown-preset list is unused dead weight.

## 2. Goals

1. Every window shows who is connected to this daemon, live, with roles at a glance.
2. Owner/admin can moderate from one surface: kick a user, grant/revoke temporary edit access.
3. A window can be a **viewer** (hands-off) or a **claimer** (owns the grid); a new **viewer
   role** can never claim without an explicit grant.
4. Workspace nav shows *where* people are working (replacing git ± counts).
5. A pinned terminal can be **pinned to a fixed size** so everyone sees the same grid
   (education/presentation); small windows scale down instead of fighting.
6. Timer becomes a simple play/pause/stop stopwatch.

## 3. Non-goals (V1)

- Profile-image avatars — **decreed deferred** until K2 Connect Cloud + Federation Groups exist
  (memory `k2-connect-cloud-access-roadmap`). V1 avatars are initials-only with role-colored
  borders; the component keeps an `imageUrl` prop slot for later.
- Per-window kick (V1 kick is per-user; session ids are added now, surfaced later).
- Cursor-position/co-editing presence inside a terminal. Presence is window/workspace-level.
- Federation-peer presence (peers are daemons, not humans; out of scope).
- Persistence of grants (grants are ephemeral by design — they die with the connection).

## 4. Roles & permissions model

New role: `Viewer` (below Member) in `Role` (`connect_users.rs:62`), wire string `"viewer"`.
Serde default remains Member (existing rows untouched). Renderer `K2Role` + capability gate
via `useServerSupports`.

| Capability | Owner | Admin | Member | Viewer |
|---|---|---|---|---|
| Appear in roster | ✓ (synthesized) | ✓ | ✓ | ✓ |
| Claim a terminal (claimer mode) | ✓ | ✓ | ✓ | only while granted |
| Type / resize while in claimer mode | ✓ | ✓ | ✓ | only while granted |
| Viewer mode (default for non-owner windows) | n/a (local defaults claimer) | ✓ | ✓ | ✓ (forced) |
| Kick users | ✓ | ✓ (not owner/admin, per `can_act_on`) | ✗ | ✗ |
| Toggle edit grants | ✓ | ✓ | ✗ | ✗ |
| Pin/unpin terminal size | ✓ | ✓ | ✓ (claimer) | ✗ |

**Mode vs grant:** claimer/viewer is a **per-window mode** (top-right toggle); the edit grant is
a **per-user permission**. A window may claim iff `role >= Member` OR (`role == Viewer` AND
user currently granted). Enforced daemon-side; UI is advisory.

**Grant lifecycle (decided):** toggle in the presence modal (grant AND revoke); auto-revoked
when the user's **last** connection deregisters; owner/admin may re-toggle any time. No timers.

**Viewer mode semantics (decided):** no PTY input AND no resize. Claimer mode grants both.

## 5. Feature specs

### 5.1 Stopwatch (S0)

- Idle state = single play button (clock icon). Click → `startTimer()` (already count-up).
- Running state = existing pause/resume/stop cluster + elapsed readout, recolored neutral.
- DELETE: `DURATION_PRESETS` + idle preset list, countdown-expiry effect, `ExtendTimerDialog`,
  `startWithDuration`/`targetDurationMs`, **CountdownOverlay + timer-themes + their settings
  entries (decided: delete)**. KEEP: `MemoDialog`, `sync:timer` cross-window broadcast,
  `time_entries` pipeline. Zero daemon changes.

### 5.2 Presence roster + modal (S1/S2/S3/S4)

- **Roster** sits in the TopBar RIGHT group, left of the stopwatch (twin in FocusLayout):
  up to 10 `PresenceAvatar` chips, then `+N` overflow chip. One chip per USER (window count
  aggregated); local owner always present, displayed as Owner.
- **PresenceAvatar**: circular initial, role-colored 2px border — **owner=amber-gold,
  admin=purple, member=blue, viewer=gray (decided)**. Optional `imageUrl` prop, unused in V1.
- **Modal** (click roster): all connected users — avatar, username, role, window count,
  workspaces being viewed, connected-since. Per row (owner/admin only, `can_act_on`-filtered):
  **Kick** button; **Edit-grant toggle** (viewer-role rows only). Live-updates from events.
- **Kick** = `revoke_user_sessions(username)` + registry walks that user's live sockets and
  closes them immediately (no 5s re-auth wait). Victim's next connect attempt fails auth.

### 5.3 Viewer/claimer toggle + enforcement (S5)

- Top-right TopBar icon (after panel toggles): flips THIS window's mode. Members default to
  **viewer** on remote windows; local owner windows default claimer. Viewer-role users see the
  toggle disabled unless granted.
- Daemon enforcement in the grid WS: connection auth class + resolved identity threaded into
  per-subscriber state; Input handler (`sessions_grid_ws.rs:833`) drops input + suppresses the
  claim-steal/snap-resize for non-claimer connections; Resize gate (`:901`) and SetActive
  likewise refuse non-claimers.

### 5.4 Workspace-nav presence (S6)

- Replace `AgentOrDiffStats` at `Sidebar.tsx:353` (project row) and `:449` (worktree row) with
  up to 3 mini avatars (~14-16px) + `+N`, joined by workspace path (prefix-match, same rule as
  `event_matches_workspace`). IconRail tooltip swaps counts for names.
- A user with multiple windows appears in every workspace they're viewing.
- **AI Commit entry point is DROPPED entirely (decided)** — delete `DiffStats`,
  `AgentOrDiffStats`, dead `AggregatedDiffStats`/`AheadBehind`/`workspacePaths`. The
  `git/changes`/`git/info` pipeline STAYS (ChangesPanel, dirty dots, branch labels).

### 5.5 Pin-to-size (S7)

- Pinned terminal tab gains a dropdown: **80×24, 100×30, 120×36, 160×48, Match my window now**
  (freezes current `active_cols/rows`), plus **Unpin**. Mount: `PaneTabBar.tsx` tab element.
- Daemon-canonical: pinned dims clamp ALL resize paths at the single chokepoint
  (`daemon_pty.rs request_resize:1088`) — grid-WS resize, typing claim-snap, SetActive,
  detach-promotion. Pin state broadcast to subscribers (label-channel pattern) so panes render
  pin UI + letterbox correctly.
- Renderer: `scaleLayout` gains a `pinnedSize` branch computing fit against the pinned grid
  regardless of `isActiveViewer`; revisit `PASSIVE_SCALE_FLOOR = 0.4` for large pins.
- Persistence: `workspace_tab_sessions` + `pinned_cols/pinned_rows/pinned_set_by`
  (**migration 0065**). Survives daemon restart.
- Permission: claimer-capable users only (see matrix). Anyone claimer-capable may unpin
  ("clears back to the juggling UX" per spec).

## 6. Architecture

**Presence registry** (new `crates/k2-daemon/src/presence.rs`, daemon-first):
`Mutex<HashMap<conn_id, PresenceEntry>>`;
`PresenceEntry { conn_id, identity: Owner | ConnectUser{username, role}, kind: AppSocket |
WorkspaceSocket{path} | GridViewer{session}, connected_at, close_handle }` +
`granted: HashSet<username>` (ephemeral).

- **"Connected" = holds the app-level `/cli/sessions/events` socket** — every window (local or
  tunneled) opens one at boot; the tunnel is transparent so remote presence is free.
- **Identity at WS upgrade**: resolve the already-present `?token=` — owner token → Owner;
  session token → `validate_session` → username + `role_for_user`. (Precedent:
  `authorize_send_message`, `http.rs:614`.) Key on the existing `subscriber_id` AtomicU64.
- **Per-workspace viewing**: record the `?path=` the per-workspace sockets already send.
- **Liveness**: server-originated WS ping on the events socket (~10s, reap after 2 misses) —
  clean closes are instant; hard drops revoke grants within ~20s.
- **Broadcast**: new app-level `SessionEvent::PresenceChanged { roster }` (whole-set,
  last-write-wins — the ActiveChanged convention) + snapshot `GET /cli/presence/roster`
  fetched on `hello` (reconnect-reconcile pattern).

**New routes** (mutating = POST-only guard at handler top, per house rule):

| Route | Method | Gate |
|---|---|---|
| `/cli/presence/roster` | GET | authorized (owner or session) |
| `/cli/presence/kick` | POST | `require_owner_or_admin` + `can_act_on` |
| `/cli/presence/grant` | POST | `require_owner_or_admin` |
| `/cli/terminal/pin-size` (set/clear) | POST | authorized + claimer-capable check |

**Data-model deltas**: `Role::Viewer` variant; `SessionRecord.session_id` field (populated at
`create_session`, surfaced later for per-window kick); migration `0065_pinned_size.sql`.

## 7. Build order

Waves (subagent worktree → cherry-pick → verify → reap, per house convention):

1. **[S0 stopwatch, S1 presence substrate]** — independent; S0 renderer-only, S1 daemon-only,
   headless-testable (two tokens, roster curl, drop-one broadcast assert).
2. **[S2 roster UI + modal (read-only), S7-daemon pin clamp + migration]**
3. **[S3 kick, S4 viewer role + grants]**
4. **[S5 claimer enforcement + toggle, S6 nav presence swap, S7-UI dropdown + scaleLayout]**

Dependencies: S1→S2→S3/S4→S5; S6 needs S1 + S2's avatar; S7 independent until its UI wave.

## 8. Acceptance / testing

- Daemon integration tests (`*_integration` naming): roster over two tokens; kick closes live
  sockets immediately; grant lifecycle incl. last-disconnect auto-revoke; pin clamp holds across
  all four resize paths; viewer-mode input dropped + claim not stolen; POST-only guards.
- Renderer vitest: presence store aggregation (per-user dedupe, overflow math); stopwatch store
  after deletions (`timer.test.ts` entries CRUD must stay green).
- Live e2e (protocol-based, dev daemon): two local windows + one K2 Connect remote login —
  roster correctness, kick, grant/revoke on disconnect; classroom scenario: pin 100×30,
  small-window viewer letterboxes, teacher kicks a student.
- Baselines at plan time: tsc 67 pre-existing; vitest 931; k2-core lib 1184; known env-flaky
  daemon tests (cell_uds, connect_users logout, v1_sandboxes::policy) pass solo.

## 9. Future (out of V1, aligned with the access roadmap)

- **K2 Connect Cloud identity**: profile images via Cloud accounts; email invites; Cloud-login
  access grants. `PresenceAvatar.imageUrl` is the drop-in point.
- **Federation Groups**: a server managed by a federation grants its user group instant access —
  presence identity gains a third source; keep username as local key, don't assume local rows.
- Per-window kick (session ids already recorded); presence history/audit; idle-vs-active states.

## 10. Decisions log

| Date | Decision |
|---|---|
| 2026-07-04 | Grants are a toggle, auto-revoked on disconnect; owner/admin re-toggle manually (Rosson). |
| 2026-07-04 | Role-colored borders on all presence chips (Rosson). |
| 2026-07-04 | Avatars initials-only V1; images wait for K2 Connect Cloud (Rosson). |
| 2026-07-04 | AI Commit nav entry point dropped entirely (Rosson). |
| 2026-07-04 | CountdownOverlay + themes deleted with the countdown (Rosson). |
| 2026-07-04 | Colors: owner=amber-gold, admin=purple, member=blue, viewer=gray (Rosson). |
| 2026-07-04 | Viewer mode = no input AND no resize (Rosson). |
| 2026-07-04 | Claimer is per-window mode; grants are per-user (Claude, unobjected). |
| 2026-07-04 | "Connected" = holds the app-level events socket; owner synthesized in roster (Claude, unobjected). |
