# Activity-detection study — stuck-busy diagnosis + orca comparison

**Date:** 2026-07-02 · **Bug:** sidebar braille spinner "sometimes spins forever — thinks the workspace is always busy" even when the agent is clearly done.
**Verdict up front:** the spinner is a *renderer-owned* state whose only idle-writers live inside a mounted, streaming `TerminalPane`. Any pane unmounted (workspace switch, tab close, retainer eviction) within ~1s of a working signal freezes `paneStatuses[terminalId]='working'` forever, because the periodic cleanup that used to decay it was retired with push-primary polling. Kessel (v2) sessions get **no hook events at all** (the hook env is never injected into daemon-PTY spawns), so there is no daemon-side rescue. Fix: move busy/idle to the daemon, where a per-session always-on task already exists.

---

## Part 1 — K2's activity pipeline, end to end

### 1.1 Renderer-side detection (`src/renderer/kessel-term/TerminalPane.tsx`)

Two working=TRUE writers, three working=FALSE writers. All five live inside the mounted pane and (except the watcher) require an **open grid-WS**.

| Signal | Where | What it writes |
|---|---|---|
| Per-frame heartbeat | `recordActivityFromSnapshot` — TerminalPane.tsx:945-947 calls `recordOutput(terminalId)` on every grid snapshot/delta | bumps `outputTimestamps` only (not status) |
| Viewport text scan | TerminalPane.tsx:949-963 builds a row→text map from the **whole viewport** and calls `detectWorkingSignal` | on match: `lastSeenWorkingAtRef = now` + `recordTitleActivity(terminalId, true)` |
| Title working marker | `case 'title'` handler TerminalPane.tsx:1512-1541 — braille prefix `/^[⠀-⣿]/` (line 1522) | `recordTitleActivity(true)` (1538-1540) |
| Title idle marker | same handler — ✱-family prefix `/^[*✱✲✳✴✵✶✷✸✹⚹⁎∗※]/` (line 1521) | `recordTitleActivity(false)` (1535-1537) |
| Bell | `case 'bell'` TerminalPane.tsx:1578-1589 — "definitive idle transition" (Claude/Codex ring on done) | `recordTitleActivity(false)` |
| Idle watcher | 500ms `setInterval`, TerminalPane.tsx:1038-1051 — if `now - lastSeenWorkingAtRef > IDLE_GRACE_MS (1000)` | `recordTitleActivity(false)` |

The scan phrases (`src/renderer/lib/agent-signals.ts:22-38`): `'esc to interrupt'` (claude/codex), `'esc to cancel'` (gemini), `'waiting for '`, `'thinking...'`, `'working...'`, `'agent is working'`, `'planning next moves'`, `'loading...'`, etc. — matched case-insensitively in the last 15 rows (`detectWorkingSignal`, agent-signals.ts:51-66).

**The in-code admission** (TerminalPane.tsx:949-959): the scan **deliberately does NOT gate on `displayOffset === 0`** — "the false-positive cost (showing 'working' while scrolled-up) is much smaller than the false-negative cost (no spinner ever)." Note agent-signals.ts:48-50 still documents the opposite ("Gated by displayOffset === 0 at the call site") — that comment is stale.

Snapshot delivery is the effect at TerminalPane.tsx:1024-1032 (`useEffect` on `snapshot`); no snapshots flow unless the grid-WS is open.

### 1.2 The store (`src/renderer/stores/active-agents.ts`)

- **Keying:** `paneStatuses: Map<paneId, PaneStatus>` where `paneId === terminalId` (e.g. `agent-chat:<projectId>`, `tab-…`); `paneProjectMap: Map<paneId, projectId>` attributes each pane to a workspace (lines 104-141).
- `recordOutput` (line 236) — timestamp only. `recordTitleActivity` (246-288) — flips idle↔working, **refuses to clear `permission`/`review`** (line 249); on first working=true, binds pane→project (264-287; P1.A parses `agent-chat:` ids so a background tick can't mis-bind to the foreground workspace). `bindPaneProject` (300) is called upfront by AgentChatPane (AgentChatPane.tsx:398, 728).
- `handleLifecycleEvent` (309-448) — the **hook** path: `start`→working, `permission`→permission+toast, `stop`→idle if pane's tab is active else `review`+toast.
- **Sidebar derivation:** `getProjectStatus(projectId)` (200-216) scans `paneStatuses`×`paneProjectMap`; consumed by ActiveBar.tsx:325-366 (`isAgentWorking = working||permission` → `<span className="braille-spinner">` at 362), Sidebar.tsx:156-165, IconRail.tsx:37/84. **The spinner reads ONLY this client store.**
- **TTL/decay: none in steady state.** The only decay is `pollOnce`'s cleanup loop (605-640: clear working/permission when `hookAge > 120s` AND `outputAge > 3s`, consts at 82-84). But since 0.39.39 push-primary (`startAgentPolling`, 702-745), when the daemon supports `daemon-broadcasts` **the 2.5s poll is GONE** — `pollOnce` runs only at startup and on events-WS reconnect (`onAppHello`, 721-723). The legacy 2.5s poll survives only as a fallback for old/remote daemons (738-745). Other resets: host switch (`__resetAgentStateForHostSwitch`, 1330-1355) and app restart. Tab close (`tabs.ts::removeTab`) touches **nothing** in this store.

### 1.3 Daemon-side canonical Active (0.39.38 / #672) — and what the daemon does NOT know

- **Active set** = daemon-owned: a workspace is Active iff `manually_active` (pinned) OR interacted within `active_window_hours` — computed by `compute_active_project_ids`, broadcast as `ActiveChanged` (`crates/k2-daemon/src/active_reaper.rs:1-52`, `recompute_and_broadcast_active` at 142). The reaper grace-reaps dormant chat PTYs Active-only (no attach gate). Client mirrors it 1:1 in `useActiveStore` (`src/renderer/stores/active.ts:47`); ActiveBar rows = mirror + `manuallyActive` pinned-to-top. **Active drives section membership and reaping — not the spinner glyph.**
- **Daemon agent-status cache**: hook lifecycle events flow through `DaemonBroadcastSink::emit` (`crates/k2-daemon/src/events.rs:68-110`) which mirrors `AgentLifecycle` onto the session-events spine as `AgentStatusChanged`; cached per pane (`session_events.rs:326-364`) and exposed as `working|idle|permission` in `/cli/ops/overview` (`ops_routes.rs:139-198`).
- **The hole:** hooks reach the daemon via `notify.sh` curling `/hook/complete?paneId=$K2SO_PANE_ID` (`src-tauri/src/agent_hooks.rs:250-295`; `k2-core/src/agent_hooks.rs:242-299`). The script **exits immediately if `$K2SO_TAB_ID` is empty**. That env is injected only by the *legacy* backend (`alacritty_backend.rs:338-350`). Kessel v2 spawns pass `env: req.env` straight through (`v2_spawn.rs:530`; hook env only under the default-OFF scoped-hooks flag, 547-602), and the renderer's spawn body carries no env (TerminalPane.tsx:1076-1095). **So for every Kessel session, hooks never fire — the daemon's status cache is empty and `handleLifecycleEvent` never runs.** The in-code comments confirm it (active-agents.ts:260-263: "for v2 panes … this is the only path that populates the map"; 614-620: "v2 panes never appear in newAgents").
- The daemon *does* receive Title/Bell for every session — alacritty emits `AlacEvent::Title/Bell` on the session's broadcast channel; the label state machine consumes Title (`daemon_pty.rs:719-746` `try_set_label_from_pty`) — but classification into working/idle happens **only in per-WS-connection loops** (`sessions_grid_ws.rs:709-750` forwards `Outbound::Title`/`Bell` to attached viewers). The per-session **grid emitter task** ("one per live session", `grid_emitter.rs:302-311`) runs viewer-independently but today handles only Wakeup/ChildExit.

### 1.4 Lifecycle interactions

- **Hidden but mounted (same-workspace background tab):** retained-view model keeps the tree mounted (`TerminalArea.tsx:166`, `PaneGroupView.tsx:180`) but the grid-WS closes on hide (`shouldHoldGridWs`, `activeViewer.ts:83`; lifecycle effect TerminalPane.tsx:1718-1771 parks it). No snapshots/title/bell arrive → the still-running idle watcher writes **idle within ~1.5s**. Result: not stuck-busy, but a *false-idle* — a hidden working agent shows no spinner (the complementary under-report bug).
- **Unmount (workspace switch / tab close / layout change):** `setActiveProject` → `restoreWorkspace` replaces the tab tree (`projects.ts:412/471/564`) → panes unmount. The unmount teardown (TerminalPane.tsx:1779-1793) closes the WS and clears timers but **never writes idle**. The store entry survives with whatever status it last had.
- **Pinned-chat retention (brand-new, commits 78557d9/698cdd6/0353044):** the pinned canonical Chat of each retained workspace is portal-hosted off-screen by `PinnedChatRetainer` (`PinnedChatRetainer.tsx:165+`; final unmount only on eviction, 345-368; crash boundary evicts too, 376-396). Retained set = MRU-by-visit ∩ canonical Active, cap `max(5, pinnedToTopCount)` (`kessel-term/retainedChat.ts:26-81`; Active-leave prunes, 110-115). A retained pane sets `retainWhileHidden` (AgentChatPane.tsx:428 — `activeProjectIds.has(projectId)`; prop at 667; predicate exemption `activeViewer.ts:60-80`, TerminalPane.tsx:468-470) so it **keeps streaming while hidden** — title/bell/snapshots/watcher all stay alive. Effect on the bug: retained pinned chats now stay *truthful* while hidden (fixes both false-idle and stuck-busy for them); **eviction** (MRU cap overflow, Active-leave/dismiss, crash) unmounts the instance and re-creates failure mode 1; non-pinned panes (Cmd+T terminals, splits, worktree chats) are unchanged.

---

## Part 2 — Stuck-busy failure modes, ranked

### FM1 — the false-writer dies with the pane ✅ REAL — primary culprit
Every idle-writer for a Kessel pane lives in the mounted `TerminalPane`; there is no unmount-writes-idle, and no hook fallback (§1.3). During active work, the viewport scan + braille title re-arm `working` on essentially **every frame**, so at any instant mid-task the last write is `working` and `lastSeenWorkingAt` is <1s old. Unmount in that window → frozen.

**Repro:** open a terminal tab, run `claude`, give it a long task; while "esc to interrupt" is showing, click another workspace in the sidebar (or Cmd+2). The old workspace's row spins forever — through agent completion, bell, even PTY exit. Only a daemon reconnect (app-hello re-poll), host switch, or app restart clears it.
**Post-retention scope:** the pinned Chat of an Active, within-cap workspace is now protected (stays mounted+streaming). Still exposed: (a) any non-pinned pane running an agent, (b) retainer **evictions** — e.g. 6+ Active workspaces, visiting the 6th evicts the LRU's chat mid-work → stuck; dismissing a workspace whose agent is working → prune → unmount → stuck.

### FM2 — store entry outlives its writers (no TTL) ✅ REAL — the enabler
`paneStatuses` is keyed per terminalId with no expiry. The decay that existed (pollOnce cleanup, active-agents.ts:605-640) became effectively dead code on modern daemons when 0.39.39 removed the 2.5s poll (702-745) — it now runs only on startup/reconnect. FM1 produces the stale entry; FM2 is why it lasts "forever" instead of ≤2.5s+grace. A closed tab's entry likewise persists (removeTab cleans nothing).

### FM3 — viewport-scan false positive ✅ REAL — aggravator (and a visible-stuck variant)
`detectWorkingSignal` matches the phrase anywhere in the last 15 viewport rows, deliberately un-gated on scroll (TerminalPane.tsx:949-959). If "esc to interrupt" is visible as *content* — scrolled-up scrollback containing a stale status-line artifact, a transcript where claude/user quoted the phrase, `WORKING_SIGNALS` source code in an editor pane — then **every** incoming frame re-arms working. While frames keep flowing (claude's idle UI does periodically repaint), the spinner sticks ON even though the agent is idle; the 1s watcher only clears it when frames stop. And any such transient false-working within 1s of a workspace switch converts into permanent FM1. Bell/✱-title write idle, but the very next frame's scan overrides them if the phrase is on screen — the scan outranks the definitive signals by frequency.

### FM4 — missed idle marker while parked ❌ KILLED (mitigated by the watcher)
True that a parked WS loses title/bell transitions and re-attach doesn't replay them (snapshot carries no title; `sessions_grid_ws.rs:791` even notes reconnect "only means missed Title/Bell lifecycle"). But the 500ms watcher keeps running in any *mounted* pane and force-idles 1s after the last working signal — a parked pane can't stay working. (It produces the opposite bug: false-idle for hidden working agents — real, but not the forever-spinner.) For *unmounted* panes this collapses into FM1.

### FM5 — daemon-Active vs client-store divergence ❌ not the spinner bug, ✅ the architectural gap
The spinner reads only the client store (§1.2); daemon Active only decides which rows exist in the Active section and what the reaper may kill. So divergence can't directly stick the spinner. The real finding is the **absence** of any daemon busy-truth for Kessel sessions (hook env never injected — §1.3): there is nothing authoritative to reconcile against, which is why every client-side lifecycle hole is fatal.

### FM6 — stuck `permission` (legacy panes) ⚠️ real but narrow
`recordTitleActivity` never clears `permission`/`review` (line 249); only a hook `stop` or the retired pollOnce cleanup can. On a legacy-backend pane, Esc-interrupting at a permission prompt (Stop hook lost) leaves red-spinner `permission` forever under push-primary. Shrinking population as Kessel takes over — but note Kessel panes can never *enter* permission, so this is legacy-only.

**Likely "spins forever" culprits, in order:** FM1 (unmount mid-work — workspace switch or retainer eviction) persisted by FM2 (no TTL), with FM3 raising the probability that the frozen state is `working`. Already mitigated: wrong-project attribution (P1.A `bindPaneProject`), host-switch leakage (`__resetAgentStateForHostSwitch`), and — for retained pinned chats only — the retention feature itself.

---

## Part 3 — How orca does it

Repo: `stablyai/orca` (Electron; cloned at `/Users/z3thon/DevProjects/Alakazam Labs/terminal-research-repos/orca`). Detection is **main-process-owned, hook-first, PTY-lifecycle-coupled**. Three channels, strictly tiered:

1. **Canonical: explicit agent hooks → normalized status.** Orca installs hooks into each CLI (`src/main/agent-hooks/installer-utils.ts:175,198` — hook curls a local HTTP listener with `paneKey=%ORCA_PANE_KEY%` from env). The transport-agnostic listener (`src/shared/agent-hook-listener.ts`) parses per-CLI payloads (Claude `UserPromptSubmit`/`PreToolUse`→working, `Stop`→done, etc. — 1202/1235/1483-1490) into four states `working | blocked | waiting | done` (`src/shared/agent-status-types.ts:10`). The header states the doctrine outright (agent-status-types.ts:1-6): *"Agent state normally comes from hooks… We still do not infer status from terminal titles anywhere in the data flow."* The main-process hook server (`src/main/agent-hooks/server.ts`) caches `lastStatusByPaneKey`, fans out over IPC, and serves a catch-up snapshot (`getStatusSnapshot`, server.ts:499-506).
2. **Lifecycle coupling + guarded fallbacks — the part K2 is missing.**
   - **PTY death clears status:** per-PTY teardown evicts the pane's cached status and notifies (`server.ts:900-933` `clearPaneCacheState` → `onPaneStatusCleared`, wired in `src/main/index.ts:955`); closed tabs are suppressed (`markTabClosedForAgentStatus`/`shouldSuppressClosedTabStatus`, server.ts:598-608). The relay does the same for SSH sessions ("Subscribe to PTY-exit events. Used by the relay-hook server to evict", `src/relay/pty-handler.ts:225`).
   - **Guarded interrupt inference:** when the user hits Esc/Ctrl+C in a working pane and the agent misses its cancellation hook, the renderer reports the *input intent* and main synthesizes `done{interrupted}` — but only under a strict baseline match (same state/prompt/agent/timestamps, per-CLI key semantics, not stale) so a delayed timer can never clobber a newer hook (`server.ts:508-570` `inferInterrupt`; renderer half in `src/renderer/src/components/terminal-pane/agent-interrupt-inference.ts`).
   - **Staleness decay:** `AGENT_STATUS_STALE_AFTER_MS = 30min` (agent-status-types.ts:218-225) — sidebar dots decay "working"→"active" when the hook stream goes silent.
3. **Secondary: OSC-title heuristics + an in-band OSC protocol — processed centrally in main on raw bytes.** `OrcaRuntimeService.onData` runs per PTY chunk regardless of viewers (`src/main/runtime/orca-runtime.ts:5040-5072`): extracts the last OSC 0/1/2 title (stateful across chunk splits, 5259-5272), classifies via glyph tables (`src/shared/agent-detection.ts:31-41` — claude ✳ idle / braille working, gemini ✦/⏲/◇/✋, keyword rules) for **sidebar badges, notifications, unread markers, stats** — never the canonical status. For CLIs whose native titles carry no state (cursor, droid, hermes), orca *synthesizes* braille titles from hook events so the title-driven UI still lights (agent-detection.ts:368-401). Additionally a JSON status protocol rides OSC 9999 in-band (`\x1b]9999;{"state":"working",…}\x07`, parsed+stripped per chunk — `src/shared/agent-status-osc.ts:4,30`; orca-runtime.ts:5250-5257).

**What orca does NOT do:** no renderer-side scanning of rendered grid *content* (no "esc to interrupt" phrase matching anywhere); no per-frame status writes from the render path; no busy-state whose lifetime depends on a React component or an attached viewer; PTY layers themselves (`src/main/daemon/pty-subprocess.ts`, `src/relay/pty-handler.ts`) carry **no** busy/idle logic — they just guarantee the byte stream reaches the one place (main/relay) that owns detection and eviction.

---

## Part 4 — Recommendation

### Options considered

**A. Targeted client-side patches** (~30-60 LOC, renderer only)
1. Unmount-writes-idle: in TerminalPane's unmount teardown (1779-1793), `recordTitleActivity(terminalId, false)` (skip when the unmount is a retainer *re-parent*, not a real teardown).
2. TTL sweep: a 10-15s interval in `startAgentPolling` that idles any `working` entry whose `outputTimestamps` age exceeds ~10s (restores the decay lost with the retired poll, without the HTTP poll).
3. Scan hygiene: gate `detectWorkingSignal` to bottom-anchored viewports (or require the phrase in the last 3 rows), reducing FM3.
   *Fixes the forever-spinner?* Yes — (1) kills FM1 directly, (2) kills FM2 as a backstop for every path (evictions, crashes, closed tabs). But it leaves hidden non-retained agents spinner-less (FM4's false-idle), leaves truth split across N renderer stores, and leaves the daemon/companion/fleet-console blind — each future client re-implements detection.

**B. Daemon-side per-session busy state** (the daemon-first answer)
The seam already exists: the **grid emitter task is one always-on task per live session** (`grid_emitter.rs:302-311`), subscribed to the same `AlacEvent` channel that carries Title/Bell/Wakeup/ChildExit — for ALL sessions, viewers or not. Add a per-session `busy: working|idle` fed by: title braille/✱ classification (the exact regexes from TerminalPane.tsx:1521-1522, moved server-side), Bell→idle, output cadence (Wakeup), optional `WORKING_SIGNALS` scan of the bottom rows at encode time (the emitter already holds the term lock per frame), a daemon-side 1-2s idle-grace timer, and **hard falses on `ChildExit` and `v2_session_map::unregister`** — the false-writer structurally cannot die before the true-writer because both live and die with the PTY. Surface it on the existing spine as `AgentStatusChanged` (session_events.rs:188-199 — the renderer's `onAgentStatusChanged → handleLifecycleEvent` path already consumes exactly this shape), with cwd→projectId resolved daemon-side as `/cli/ops/overview` already does (ops_routes.rs:186-193).
*Fixes the forever-spinner?* Structurally: busy-state lifetime == session lifetime; unmounts, evictions, parked WSes, and workspace switches become irrelevant to truth. Also fixes the false-idle for hidden agents, gives companion/fleet/`k2 talk` the same truth for free, and matches both the K2 daemon-first principle and orca's proven shape (status owned by the process-owning side, cleared on PTY teardown, renderer display-only).

### Recommendation: B, with A's two cheapest patches as slice 1

**Slice 1 — renderer hotfix (ship immediately):** unmount-writes-idle + the 10s staleness sweep in `active-agents.ts`. Two small diffs; forever-spinner dies today; no protocol change; harmless once slice 3 lands.
**Slice 2 — daemon busy state:** per-session `working/idle` in the grid-emitter/daemon_pty seam (title + bell + output-cadence + child-exit + unregister + idle-grace; text-scan behind a flag), cached like the existing agent-status cache, emitted as `AgentStatusChanged{paneId=sessionId, status=start|stop}` on the session-events spine, snapshot in `/cli/ops/overview` and app-hello. Test headless: spawn a claude session with no WS attached, assert start/stop events fire (per `feedback_daemon_first`).
**Slice 3 — renderer becomes a display:** sidebar spinner reads the daemon-pushed busy map (project attribution daemon-side); delete `recordTitleActivity` writers from TerminalPane (or demote the viewport scan to a dev-only diagnostic); keep `manuallyActive`/Active-section logic untouched. Follow-up (separate, valuable): inject `K2_PANE_ID`/`K2_TAB_ID`/hook env into Kessel v2 spawns so real lifecycle hooks (start/stop/permission) come back as the *primary* signal with title/bell/scan as fallback — that is the orca doctrine, and it restores `permission` states that Kessel panes currently can never show.
