# Pinned-Chat Background Render — Design Note

**Goal:** workspace switching feels instant for the tab that matters — the pinned
canonical Chat tab — with strictly bounded background cost. Everything else keeps
parking exactly as today.

**Proposal (owner, restated):** the pinned canonical chat tab stays attached and
rendering while hidden, **iff** (a) its workspace is in the sidebar's Active
section and (b) the session exists on the daemon. All other panes keep the
park-on-hidden behavior (grid-WS closes, rendering stops).

**Status of this note:** design only, directly buildable. All file:line refs are
against the tree at `48849b3` (worktree `agent-a3bf5710d6082db8a`).

---

## 0. TL;DR

- **Load-bearing fact:** hidden workspaces' panes are **fully UNMOUNTED**, not
  hidden. `stashWorkspace` (`src/renderer/stores/tabs.ts:3839-3893`) sets
  `tabs: []` on switch-away — the comment says it outright: *"Clear active view
  (React unmounts, but PTYs stay alive in backend)"* (`tabs.ts:3885`). So "don't
  close the WS" alone cannot work across workspaces; the design needs a
  mounted-but-hidden host. (Within a workspace, tabs *are* mounted-but-hidden —
  `display:none` retained-view in `TerminalArea.tsx:164-186`.)
- **Recommended mechanism:** a per-window **PinnedChatRetainer** that owns one
  `AgentChatPane` instance per exempt workspace and **portals** it into the
  visible tab's pane slot when that workspace is foreground, or into a hidden
  off-screen container when it isn't. Plus one new input on the pure
  `shouldHoldGridWs` predicate (`retainWhileHidden`) so the retained pane keeps
  its grid-WS while hidden. `TerminalPane` internals otherwise untouched.
  Precedent for app-level hidden hosting already exists:
  `BackgroundTerminalSpawner.tsx` (spawn-only hidden panes under
  `TabVisibilityContext.Provider value={false}`).
- **"Keep rendering" is nearly free** because the v2 pane already renders only a
  viewport window (~rows visible rows, `TerminalPane.tsx:2047-2065`), rows are
  memoized (`TerminalPane.tsx:224-240`), and a `display:none` subtree pays no
  layout/paint. The real memory cost is the in-memory snapshot mirror
  (scrollback-capped at 5000 rows), which exists in *any* variant that can paint
  instantly. So we take the owner's literal "keep rendering"; the
  warm-state/cold-pixels variant is kept as an optional follow-up knob, not the
  core mechanism (see §2.4).
- **Reaper:** non-issue by construction (owner's observation confirmed in code).
  The daemon Active reaper is **Active-gated only — there is deliberately no
  subscriber/attach gate** (`crates/k2-daemon/src/active_reaper.rs:12-23`).
  A retained background attachment neither blocks nor is threatened by the
  reaper while the workspace is Active — and the exemption predicate *requires*
  Active. Footnote in §1.5.

---

## 1. Current mechanics (the map)

### 1.1 Park-on-hidden machinery (0.39.13 spawn ⊥ stream)

`src/renderer/terminal-v2/TerminalPane.tsx`:

- **Spawn effect** (`:830-1045`, deps `[terminalId, cwd, command, args, reconnectAttempt]`
  — deliberately NO `isTabVisible`): idempotent POST `/cli/sessions/v2/spawn`,
  stashes `sessionIdRef`, bumps `spawnGeneration`. Never opens a grid-WS.
- **Grid-WS lifecycle effect** (`:1424-1470`, deps `[spawnGeneration, isTabVisible]`):
  the ONLY place the grid-WS opens/closes. Decision is the pure predicate
  `shouldHoldGridWs({visible, exited})`
  (`src/renderer/terminal-v2/activeViewer.ts:73-75`): *visible AND not exited*.
  Hidden ⇒ close the socket, set phase `'parked'` (`:1460-1464`).
- **`'parked'` phase** (`:417-423`): PTY warm on the daemon, no grid-WS, no
  frames. Becoming visible re-runs the effect ⇒ `openGridWs()` (`:1053-1160`)
  ⇒ WS handshake ⇒ daemon sends a **full snapshot (grid + scrollback) on every
  connect** (`crates/k2-daemon/src/sessions_grid_ws.rs:64`) ⇒ paint.
- **Reconnect-on-drop** (`ws.onclose`, `:1359-1406`) also gates on
  `shouldHoldGridWs` (`:1379-1384`), so hidden/exited panes never reconnect.
- **Unmount teardown** (`:1478-1493`, `[]`-deps): closes the WS, cancels rAF.
  PTY survives (explicit close only via `tabs.ts removeTab` → `v2/close`).
- **Frame pipeline:** WS messages queue and apply once per rAF
  (`:557-595`); `mergeDelta` is pure (`:323-355`); only damaged rows re-render
  (memoized `TerminalRow`, `:224-240`); only the **viewport window** is in the
  DOM (`visibleRows` memo, `:2047-2065`) — scrollback lives in JS state, not DOM.

**Visibility source:** `useIsTabVisible()` /
`src/renderer/contexts/TabVisibilityContext.tsx:13-17` (default `true` outside
tab wrappers). Provided at two nesting levels:

- **Tab level:** `TabGroupColumn` in
  `src/renderer/components/Terminal/TerminalArea.tsx:164-186` — the retained-view
  model: *every open tab of the ACTIVE workspace stays mounted*; only the active
  tab is `display:block`, the rest `display:none` (`:179`), each wrapped in
  `TabVisibilityContext.Provider value={isActiveTab}` (`:175`).
- **Pane-item level:** `PaneGroupView.tsx:161, 300-309` narrows visibility per
  pane item within a tab.

### 1.2 THE LOAD-BEARING FACT — workspace switch fully unmounts

Workspace switching is **not** visibility-gating. The projects store
(`src/renderer/stores/projects.ts:350-412, 512-560, 612-646`) runs
`stashWorkspace(oldKey)` → `restoreWorkspace(newKey)`:

- `stashWorkspace` (`tabs.ts:3839-3893`) moves the live `tabs`/`extraGroups`
  into `backgroundWorkspaces[key]` and sets `tabs: [], activeTabId: null, …` —
  **React unmounts every pane of the outgoing workspace** (PTYs survive on the
  daemon). It also tears down the single per-active-workspace session-events WS
  (`:3852-3854`).
- `restoreWorkspace` (`tabs.ts:3895-3967`) swaps the stashed snapshot back in
  (fast path) or reloads the serialized layout (slow path), then the projects
  store re-runs `ensurePinnedAgentTabForMode` (`tabs.ts:946-1032`).

**Consequence:** on switch-back, the pinned chat's whole component chain
remounts from scratch and pays, sequentially:

1. `AgentChatPane` mounts → `POST /cli/workspace/ensure-pinned-chat` (1 RTT;
   phase `'ensuring'` blocks render of the terminal —
   `AgentChatPane.tsx:468-474, 603-609`);
2. `TerminalPane` mounts → `POST /cli/sessions/v2/spawn` (1 RTT, idempotent
   attach via `attachAgentName=projectId`);
3. grid-WS handshake (TCP+TLS+upgrade through the tunnel when remote);
4. full snapshot transfer (grid + up to 5000 scrollback rows), then first paint.

This is why "switch back to a remote workspace" visibly lags, and why the fix
cannot be *only* "don't close the WS": there is no WS to keep — the component
that owned it is gone. **"Keep attached while hidden" requires
mounted-but-hidden hosting across workspace switches.**

### 1.3 The pinned canonical chat tab, structurally

- The Chat tab is the **pinned system agent tab**: created/reconciled by
  `ensurePinnedAgentTabForMode` → `ensureSystemAgentTabs`
  (`tabs.ts:946-1032`, `:879`), flagged `isSystemAgent`, ordered before regular
  tabs. Its pane item is `item.type === 'agent'` rendered by
  `PaneGroupView.tsx:286-309` as `AgentChatPane`.
- `AgentChatPane` (`src/renderer/components/AgentPane/AgentChatPane.tsx:72-125`)
  keys a clean remount per workspace (`key={projectId}:{daemon|legacy}`) and
  splits on the `daemon-pinned-chat` capability:
  - **Daemon-owned path** (`AgentChatTerminalDaemon`, `:378-638`, 0.39.39 #683):
    mount → `ensure-pinned-chat` (find-or-spawn; daemon resolves resume-vs-fresh
    from `workspace_sessions` — identity SSOT per 0.39.40) → render
    `TerminalPane` with `attachAgentName={projectId}` (`:626`) and **no
    command/args** — the idempotent v2 spawn *attaches* to the PTY the daemon
    spawned under the canonical workspace key. Session lifecycle events
    (`SessionAdded`/`SessionRemoved`) arrive on a **per-pane** session-events WS
    (`subscribeToWorkspaceSessionEvents`, `session-events.ts:245` — each call
    opens its own socket scoped to the projectPath) and drive
    re-attach (`attachNonce` remount) / idle.
  - **Legacy fallback** (`:684-1038`): renderer-orchestrated resolve + breaker;
    only used when the host daemon lacks the capability.
- Difference vs an ordinary terminal tab: ordinary tabs derive
  `agent_name = tab-${terminalId}` and own their spawn args; the pinned chat
  attaches to a daemon-owned session keyed by `projectId`
  (`TerminalPane.tsx:373-380, 832-837`).

### 1.4 "Active section"

- Canonical Active is **daemon-owned** (#672, 0.39.38): a workspace is Active
  iff pinned (`manually_active`) or interacted-with inside
  `active_window_hours`. The renderer mirrors it 1:1 in `useActiveStore`
  (`src/renderer/stores/active.ts:24-82`) via `GET /cli/projects/active`
  snapshot + full-set `active_changed` deltas over the session-events WS
  (`session-events.ts:591`). The sidebar Active section renders from this
  mirror: `src/renderer/components/Sidebar/ActiveBar.tsx` (capability-gated;
  local-window fallback for old daemons).
- Transitions: opening/focusing a workspace POSTs `projects/activate`
  (dedup-guarded, `projects.ts:47-62`); aging out of the window or explicit
  dismiss removes it (reaper recomputes + broadcasts,
  `active_reaper.rs:140-153`).

### 1.5 The daemon reaper — verified, and why it's a footnote here

`crates/k2-daemon/src/active_reaper.rs` (module docs `:1-51`):

- Reap-eligible iff `!in_active_set(projectId)` AND `!heartbeat_enabled`
  (`:12-16`). **"There is no subscriber/attach gate. Attachment does NOT keep a
  session alive; *Active* does."** (`:18-23`) — `subscriber_count()`
  (`crates/k2-core/src/terminal/daemon_pty.rs:825-827`) is data, never a reap
  input. 15s grace (`:68`), 30s tick (`:72`), fire-time re-check, then
  force-close via `v2_session_map::unregister` (`:419-425`) which emits
  `SessionRemoved` and `kill()`s the child
  (`crates/k2-daemon/src/v2_session_map.rs:200-285`).
- Scope: the reaper closes **dormant workspace chat PTYs** (canonical
  sessions). Ordinary `tab-*` sessions are never touched by it — they live
  until their tab is explicitly closed (`removeTab` → `v2/close`) or the
  background workspace is explicitly cleared
  (`clearBackgroundWorkspace`, `tabs.ts:3999`).

**Owner's observation confirmed:** a canonical pinned session is effectively
immortal *while its workspace is Active* — and our exemption predicate requires
Active membership. Therefore:

- A retained background attachment **cannot** pin a session past its Active
  window (no subscriber gate) — the original "background attachment defeats the
  reaper" worry does not exist in this codebase.
- When the workspace ages out of Active, the `active_changed` delta shrinks the
  retained set renderer-side (we detach), and the reaper independently
  force-closes after its grace. If our detach loses the race, the retained pane
  simply observes `child_exit` → phase `'exited'` → no reconnect
  (`TerminalPane.tsx:1319-1342, 1379-1384`) → retainer evicts. Existing
  handling covers it; **no reaper change needed, and none proposed.**

**Position:** keep the reaper exactly as-is. Retaining/non-retaining subscriber
distinctions would re-introduce the attach-gate the #672 design explicitly
removed, for zero benefit under the Active-∧-retained predicate.

### 1.6 What reattach costs (remote), and what an idle attachment costs

Reattach on switch-back today (the §1.2 chain), over K2 Connect:

- 2 sequential HTTP RTTs (`ensure-pinned-chat`, `v2/spawn`) + WS upgrade
  (~1.5-2 RTTs incl. TCP/TLS through the relay) + full-snapshot transfer +
  1 rAF to paint. At 60-150 ms tunnel RTT that's ~400-800 ms of pure
  round-trips **before** the snapshot bytes move.
- Snapshot size: grid + full scrollback mirror, capped at
  `SCROLLBACK_CAP = 5000` rows (`daemon_pty.rs:59`). Since the k1 wire trim
  (`ce54afb`: trailing-space trimming — rows no longer padded to full width),
  a full snapshot is roughly **7.8× smaller** than the padded form
  (owner-measured); a chat session with a fat scrollback still ships
  ~100 KB-1 MB JSON. On a 20 Mbps uplink 1 MB ≈ 400 ms. Total switch-back:
  **~0.5-1.5 s remote** (matches felt experience), ~100-250 ms local.
- Daemon-side stages are already logged: `[v2-perf] side=daemon
  CONNECT-SUMMARY … ws_accept_ms first_snap_ms rows cols scrollback`
  (`sessions_grid_ws.rs:417-423`); renderer stages `creds/spawn_fetch/ws_open/
  first_snapshot/first_render` (`TerminalPane.tsx:666-681, 1501-1536`).

Idle background attachment (what the exemption buys, cost side):

- The shared per-session **grid emitter** (0.39.46,
  `crates/k2-daemon/src/grid_emitter.rs`) consumes damage once, encodes once,
  broadcasts `Arc<str>` frames to N subscribers — a background subscriber adds
  **no extra encode**, only socket bytes. Emission is Wakeup-driven with a 16 ms
  floor (`:120-132`): an **idle session sends zero frames** — the standing cost
  is WS keepalive noise, effectively nil over the tunnel.
- A busy background session streams paced deltas (damaged rows only, ≥16 ms
  apart, `FRAMES_CAP=256` with Lagged→fresh-snapshot recovery,
  `grid_emitter.rs:74-77`, `sessions_grid_ws.rs:486`). Renderer-side, hidden
  frames cost `mergeDelta` + reconciliation of memoized rows in a
  `display:none` subtree — no layout, no paint. (Note: there is no ack-gated
  per-client pacing today; the bound is the 16 ms emitter floor + broadcast-lag
  snapshot recovery + TCP backpressure. Good enough for ≤3 retained panes.)

### 1.7 Multi-viewer safety — verified

A hidden-but-attached pane cannot fight the visible viewer:

- **Active claim:** `computeDesiredActive` requires
  `visible && paneFocused && windowFocused` (`activeViewer.ts:33-35`); the
  recompute paths all route through it (`TerminalPane.tsx:1771-1779,
  1871-1873`), send-level deduped (`:1712-1762`), and the WS-reconnect re-prime
  uses the same full predicate (`:1182-1189`). Hidden ⇒ `set_active:false`.
- **Resize:** client-side, `sendResize` is window-focus-gated (`:1659-1665`)
  and the ResizeObserver drops zero-rect / <10×3 / unchanged dims
  (`:2404-2419` — the kessel "no-op resize is not free" lesson is already
  encoded). Daemon-side, resize frames are accepted **only from the active
  subscriber** (`:1647-1653`) — hard enforcement even if a hidden pane
  misbehaved.
- Cross-remount re-claim suppression (0.39.43) keeps a retained pane's remount
  from stealing the active slot from a remote viewer
  (`activeViewer.ts:77-137`).

### 1.8 Prior art in-tree and out

- `BackgroundTerminalSpawner.tsx` — an app-level, off-screen,
  `TabVisibilityContext=false` host for TerminalPanes (spawn-only, self-removing
  after 2 s). The retainer below is its streaming sibling.
- Orca study (`.k2/notes/orca-pty-mobile-study.md`): soft-leave grace (250 ms)
  and snapshot-on-attach for background viewers; their "passive viewer never
  claims active/resize" maps exactly onto our hidden-pane predicate. Their
  restore-on-detach gap does not bite here (retained panes never resize).
- `kessel-hard-learnings.md` §2.6/§2.8: skip no-op resizes; don't add wipe
  hacks — both respected (re-show triggers a ResizeObserver pass whose dims are
  unchanged ⇒ deduped to nothing).

---

## 2. The design

### 2.1 The exemption predicate — exactly

A pane is **retained** (attached + rendering while hidden) iff ALL of:

1. **Pinned canonical chat** — the `AgentChatPane` of the workspace's system
   agent tab (never ordinary terminals, never Inbox, never file panes).
2. **Workspace ∈ Active** — `useActiveStore.activeProjectIds.has(projectId)`
   (daemon-canonical mirror). Foreground workspace trivially qualifies (opening
   IS activation, PRD §4.3.1).
3. **Session exists on the daemon** — operationally: the retained
   `AgentChatPane` is in phase `'ready'` and its `TerminalPane` is not
   `'exited'`/`'error'`. `SessionRemoved` / `child_exit` ⇒ evict. (Per owner
   correction: no separate "not reaped/dormant" clause — §1.5 makes it
   redundant with clause 2.)
4. **Within the cap** — at most `MAX_RETAINED_BACKGROUND` (default **3**)
   background workspaces, evicted LRU by last-foregrounded time (§2.5).
5. **Same connect-host** — implicit and decided by architecture: the app
   remounts wholesale on host switch (`<App key={hostKey}>`, #625 —
   `host-switch-reset.test.ts:1-19`), which unmounts the retainer and drops all
   background attachments. Background attachments **do not span hosts**; the
   old host's WS would be unreachable anyway. Also gate on the
   `daemon-pinned-chat` capability (retain only the daemon-owned path; legacy
   fallback keeps today's behavior).

Expressed as a pure, unit-testable function (mirroring `activeViewer.ts`):

```ts
// src/renderer/terminal-v2/retainedChat.ts
export interface RetainedChatInputs {
  candidates: Array<{ projectId: string; lastForegroundedAt: number }>
  foregroundProjectId: string | null
  activeProjectIds: ReadonlySet<string>
  maxBackground: number            // default 3
}
/** Returns the set of projectIds whose pinned chat stays attached+rendering. */
export function computeRetainedChatSet(i: RetainedChatInputs): Set<string>
```

Foreground is always in the returned set (it's the visible pane); background
membership = Active ∩ candidates, most-recently-foregrounded first, capped.

### 2.2 Mechanism — chosen: retained pane host + portal ("keep the instance alive")

Because §1.2 says the pane is unmounted on switch, the smallest change that
preserves the WS is to move **ownership** of the pinned-chat React instance out
of the per-workspace tab tree into a per-window host that survives switches:

- **`PinnedChatRetainer`** (new, mounted once inside `App`, sibling of
  `BackgroundTerminalSpawner`): for each entry in the retained set, renders

  ```tsx
  <RetainerErrorBoundary key={projectId}>
    <TabVisibilityContext.Provider value={slotVisible}>
      {createPortal(<AgentChatPane agentName… projectPath… />, slotEl ?? hiddenHostEl)}
    </TabVisibilityContext.Provider>
  </RetainerErrorBoundary>
  ```

  `hiddenHostEl` is an off-screen `display:none`/`opacity:0` container (same
  pattern as `BackgroundTerminalSpawner.tsx:35-49`). Changing a portal's
  container **moves the DOM without unmounting the component** — state, WS,
  scroll offset, snapshot all survive; re-show is literally "the same DOM
  appears in the tab slot".
- **`PinnedChatSlot`** (new): what `PaneGroupView.tsx:286-309` renders for the
  `agent` item *when the feature + capability are on* — an empty `div` that
  registers `{projectId, el, visible: useIsTabVisible()}` in a small
  `useRetainedChatStore` on mount/visibility-change and unregisters on unmount
  (stash). When the feature is off or the workspace isn't retainable (legacy
  daemon path), PaneGroupView renders `AgentChatPane` inline exactly as today.
- **Ownership rule:** the retainer ALWAYS owns the `AgentChatPane` instance for
  retainable workspaces — foreground or background — so there is a single code
  path and no instance hand-off. Candidates enter the retained set when their
  workspace is first foregrounded (retain-on-visit; no eager pre-attach of
  never-visited Active workspaces — open question Q2), and update
  `lastForegroundedAt` on each visit.
- **`shouldHoldGridWs` grows the exemption input** (the owner's framing):

  ```ts
  export interface GridWsInputs {
    visible: boolean
    exited: boolean
    /** Pinned-canonical-chat exemption: hold the grid-WS while hidden. */
    retainWhileHidden?: boolean
  }
  export function shouldHoldGridWs(i: GridWsInputs): boolean {
    return (i.visible || i.retainWhileHidden === true) && !i.exited
  }
  ```

  Threaded as a new optional `TerminalPane` prop (`retainWhileHidden`), mirrored
  into a ref like `tabVisibleRef`, added to the grid-WS effect's deps
  (`TerminalPane.tsx:1424-1470`), to the `ws.onclose` reconnect gate
  (`:1379-1384`), and to `openGridWs`'s `isStale()` check (`:1070-1072` — today
  it aborts a mid-handshake connect when `appliedVisibleRef !== true`; it must
  accept "hidden but retained"). `AgentChatPane` passes
  `retainWhileHidden={isRetained}`. Every other consumer passes nothing —
  byte-identical behavior.

**Why not "warm state, cold pixels" as the core mechanism?** The
state-cache-outside-React variant (a headless module that adopts the WS +
merges deltas into a snapshot, from which a remounting TerminalPane hydrates)
was seriously weighed. It avoids portals, but:

1. it requires extracting WS ownership from a 3000-line battle-scarred
   component (handlers, reconnect, active-claim re-prime, phase machine all
   close over `wsRef`) — far larger blast radius than one predicate input;
2. it still pays a remount of `AgentChatPane` + `ensure-pinned-chat`'s blocking
   `'ensuring'` phase on every switch-back unless we also cache the ensure
   response — more new invariants;
3. the render cost it saves is small: the DOM is viewport-windowed
   (§1.1), rows are memoized, and `display:none` skips layout/paint. The
   dominant RAM cost (the snapshot mirror) is identical in both variants.

The literal "keep rendering" is therefore ~as cheap as "warm state, cold
pixels" while being dramatically simpler and also buying a real bonus: the
snapshot-driven **activity detection keeps running for retained panes**
(`TerminalPane.tsx:793-801`) — background Active workspaces get live
working/idle spinners in the sidebar, which parked panes can't provide today.

Optional follow-up knob (only if telemetry shows hidden reconciliation matters
under sustained TUI spam): gate `flushPendingFrames` on visibility — while
hidden, fold frames into `snapshotRef` via the existing pure `mergeDelta` and
defer the single `setSnapshot` to the show transition. One render from
in-memory state on re-show; zero React work while hidden. Kept out of scope for
v1.

### 2.3 What switch-back becomes

Foreground→background: stash unmounts the *slot*; the retainer's portal
container flips to `hiddenHostEl`; `TabVisibilityContext` flips false; the pane
keeps its WS (exemption) and keeps applying frames. `set_active:false` is sent
once (dedup-guarded) — correct: we stop being the resize/focus authority.

Background→foreground: restore mounts the slot; portal container flips to the
slot's `el`; visibility flips true; `recomputeAndSendActive` claims (if
focused); ResizeObserver re-fires with unchanged dims ⇒ deduped no-op. **Zero
HTTP round-trips, zero WS handshakes, zero snapshot transfer, no 'ensuring'
gate — one portal move + one React commit (< 1 frame).**

### 2.4 RAM honesty (per retained background pane)

| Component | Estimate | Notes |
|---|---|---|
| Snapshot mirror (JS) | **~1-8 MB typical, ~15 MB worst** | `scrollback` capped at 5000 rows (`daemon_pty.rs:59`); CellRun objects ≈ 100-150 B + strings; dense TUI rows are multi-run. This is THE cost. |
| DOM kept warm | ~tens of KB | viewport-windowed (~40-50 row divs), NOT 5000 rows (`TerminalPane.tsx:2047-2065`). |
| Grid-WS + rAF buffers | negligible | pending-frames queue capped at 60, flushed. |
| Session-events WS | 1 socket per retained workspace | `subscribeToWorkspaceSessionEvents` opens its own WS per pane (`session-events.ts:245`). |
| CPU while hidden | ~0 idle; busy: mergeDelta + memoized reconciliation, no layout/paint | emitter floor 16 ms; damage-only deltas. |

With the default cap of 3: **~5-25 MB aggregate typical**. Context vs the
original optimization's intent: issue-#8 parking was about (a) N hidden
grid-WS subscribers overrunning the per-session broadcast and (b) hidden panes
claiming active. Both are structurally solved since — the shared emitter
encodes once regardless of N (0.39.46), and the active claim requires
visibility (#8). Parking's remaining value is RAM + tunnel bytes, which the cap
bounds. Everything except ≤3 pinned chats still parks.

### 2.5 Bounds, eviction, failure modes

- **Cap:** `MAX_RETAINED_BACKGROUND = 3` (constant; owner may want a setting).
  Eviction order: least-recently-foregrounded first. Eviction = drop from the
  retained set ⇒ portal child unmounts ⇒ `TerminalPane` unmount teardown closes
  the WS (`:1478-1493`) ⇒ PTY stays warm on the daemon (normal park semantics,
  just unhosted). Re-visit re-enters the set (pays today's full reattach —
  same as status quo, never worse).
- **Aged out of Active:** `active_changed` delta shrinks the set ⇒ evict (same
  path). If the daemon reaper fires before/around our detach: `SessionRemoved`
  on the pane's session-events WS flips `AgentChatPane` to `'idle'`
  (`AgentChatPane.tsx:500-507`), and the grid-WS sees `child_exit` ⇒ phase
  `'exited'` ⇒ no reconnect (`:1319-1342, 1379-1384`). Both paths verified
  present; evict on either.
- **Child exit while backgrounded** (user typed `exit` remotely, crash): same
  as above — `'idle'`/`'exited'` ⇒ evict; on next visit the normal
  ensure/Retry flow runs. No auto-respawn from the background (PRD §4:
  the daemon never auto-spawns a pinned chat; we honor that).
- **Mid-flight WS drop while backgrounded:** `ws.onclose` consults
  `shouldHoldGridWs` — with the exemption it now schedules the normal backoff
  reconnect for a retained hidden pane (this is *desired*: the whole point is
  staying warm). Backoff caps at 5 s (`:1390`); if the daemon is truly gone the
  host-level daemon-reconnect machinery handles it as today.
- **Host switch:** `<App key={hostKey}>` remount kills the retainer ⇒ all
  background attachments drop. By design (§2.1.5).
- **Window close / multi-window:** retainer is per-window; two windows retain
  independently. Hidden panes never claim active (§1.7), and the daemon-side
  emitter fan-out makes extra subscribers cheap.
- **Crash containment:** each retained child wrapped in its own error boundary
  so a pane crash can't take down the retainer (pattern:
  `FocusErrorBoundary`, `App.tsx:154-179`).

### 2.6 Telemetry / how we prove the win

- **New renderer stage:** `[v2-perf] stage=show_to_painted` — measure from the
  `isTabVisible false→true` transition (and, for workspace switches, from
  `setActiveProject`'s t0 via a `[ws-switch]` mark in `projects.ts`) to the
  first post-show commit (rAF after snapshot flush). Land it in Slice 1 so we
  capture BEFORE numbers on the parked path, then compare after Slice 2.
  Expected: remote switch-back p50 from ~0.5-1.5 s → < 50 ms for retained
  chats.
- Existing counters to watch for regressions: daemon `CONNECT-SUMMARY`
  (`sessions_grid_ws.rs:417`) — retained panes should show ~zero reconnects
  per switch; `[v2-reconnect]` lines; dev badge FPS while a busy retained pane
  streams hidden.
- Dev affordance: retainer count + per-pane snapshot row counts behind the
  existing dev badge (or `localStorage.K2SO_RETAINED_VERBOSE`).

---

## 3. Build plan (PR-sized slices)

### Slice 1 — predicate + within-workspace retention (no hosting change)
*Small, ships value alone: the chat tab of the FOREGROUND workspace stays live
behind other tabs (today it parks and pays WS+snapshot on every tab switch —
the same remote cost, minus the two POSTs).*

- `activeViewer.ts`: add `retainWhileHidden` to `GridWsInputs` +
  `shouldHoldGridWs` (§2.2). Extend `activeViewer.test.ts` truth table
  (visible×exited×retain = 8 cases; assert active-claim predicate unchanged).
- `TerminalPane.tsx`: new optional prop `retainWhileHidden`; ref-mirror; thread
  into the grid-WS effect deps, `onclose` gate, and `openGridWs.isStale`.
  Add `stage=show_to_painted` perf mark on the hidden→visible transition.
- `AgentChatPane.tsx` (daemon path only): pass
  `retainWhileHidden={workspace Active}` from `useActiveStore`.
- Tests: activeViewer unit table; a TerminalPane-level test that a
  visibility flip with `retainWhileHidden` does NOT close the socket (mock WS,
  pattern from existing pane tests); manual smoke incl. remote host.

### Slice 2 — cross-workspace retainer (the headline feature)
- New `src/renderer/terminal-v2/retainedChat.ts`: `computeRetainedChatSet`
  (pure, §2.1) + `useRetainedChatStore` (retained entries, slot registry,
  `lastForegroundedAt`, cap eviction, `active_changed` subscription).
- New `PinnedChatRetainer` + `PinnedChatSlot`; mount retainer in `App`
  (both normal and focus-mode layouts); `PaneGroupView` renders the slot for
  retainable agent items (feature-flag + `daemon-pinned-chat` capability gate;
  legacy path untouched).
- Eviction wiring: `SessionRemoved`/`child_exit`/Active-shrink/cap/host-switch.
- Tests: `computeRetainedChatSet` unit table (cap, LRU order, foreground
  always-in, Active gating); tabs-store test that stash/restore with a retained
  chat keeps the slot contract (extend `tabs.test.ts` lifecycle tests);
  retainer store tests for eviction paths. Manual: 2 remote workspaces
  ping-pong, verify zero `CONNECT-SUMMARY` lines on switch-back.

### Slice 3 — telemetry, badge, and (optional) cold-pixels knob
- `[ws-switch]` switch→painted metric in `projects.ts` + before/after report.
- Dev-badge retained-pane counter + snapshot-size readout.
- Only if measurements justify: hidden-flush gating (`mergeDelta` into
  `snapshotRef` while hidden, single `setSnapshot` on show).

---

## 4. Open questions — owner only

1. **Cap value/policy:** is 3 background attachments right, and should it be a
   setting? (Each ≈ 1-8 MB + one idle WS pair; Active can grow well past 3.)
2. **Retain-on-visit vs eager:** retain only workspaces visited this app run
   (recommended, lazy), or eagerly pre-attach every Active workspace's chat on
   boot so even the FIRST visit is instant? (Eager multiplies boot-time tunnel
   snapshots by N.)
3. **Host switch:** confirm background attachments dropping on host switch is
   acceptable (architecturally forced today via `<App key={hostKey}>`); any
   appetite for cross-host retention later would be a much bigger project.
4. **Scope:** pinned Chat only, or should the pinned Inbox tab (cheap, no PTY)
   and/or worktree chats ever qualify? (This design says Chat only.)
5. **Capability gating:** OK to gate on the `daemon-pinned-chat` capability
   (0.39.39+) so legacy/old remote daemons keep exactly today's behavior?

## 5. Positions taken (for the record)

- **Reaper:** unchanged. Active-gated reaping with no attach gate is the
  correct invariant; the exemption predicate riding on Active membership means
  retention and reaping can never disagree for long, and a lost race degrades
  to today's `'exited'`/`'idle'` handling. (Owner correction folded: canonical
  sessions are effectively immortal while Active — the interaction is a
  footnote, not a decision point.)
- **Mechanism:** portal-retained instance over headless state cache — smallest
  correct change given the full-unmount fact; `TerminalPane` internals gain one
  predicate input and nothing else.
- **"Keep rendering" literally:** yes — it's nearly free given viewport
  windowing + memoized rows + `display:none`; cold-pixels is an optional knob,
  not the design.
