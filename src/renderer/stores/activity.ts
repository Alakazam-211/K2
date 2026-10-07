// Agent activity, as the daemon decided it
// (prd-daemon-activity-and-thread-working-v1 S5: RL9–RL14, A35–A36).
//
// The daemon owns "what is each agent doing" (S1–S4): one row per live v2
// session, folded into one rollup per workspace. This module is the client's
// copy of those rows, one per server (`ServerScope.id`), and nothing else
// decides activity in the renderer: no title scan, no phrase scan, no hook
// map, no idle timer. Every surface (Sidebar, Active bar, tabs, Home rows,
// Zen, the close guard) reads it.
//
//   - Fed only by `activity_changed` frames and `GET /cli/activity/snapshot`
//     (RL10). A snapshot replaces the whole view for that server.
//   - Pull rules (RL4): on attach and on every app-socket hello (a socket
//     (re)opened, which also covers a daemon restart's new `instance_id` and
//     the reconnect after a 4008 lag close), on a `{"kind":"resync"}` control
//     frame, on a frame whose `instanceId` is not the one we hold, and on a
//     `seq` gap. Frames apply in `seq` order; an older `seq` is dropped.
//     Frames that arrive while a pull is in flight wait for it.
//   - An older server without the `daemon-activity` reported feature (RL13,
//     Q5): its `session_activity_changed` stream is shown as it is, one row
//     per agent name. No merging, no scanning.
//   - "Done, unseen" (`unseenDone`) is per-client view state (RL11): set when
//     a turn the daemon ended (`turnEnded`) settles while this client isn't
//     looking at the session, cleared when it looks or the row works again.
//     The dot itself is never debounced; toasts wait 1.5 s and the chime
//     keeps today's rules (A36).
//
// The Active touch on lead → working is the daemon's (RL12, A33): nothing
// here writes to the server.

import { createStore, type StoreApi } from 'zustand/vanilla'
import { useStore } from 'zustand'
import { daemonCliGet } from '@/lib/daemon-cli'
import { playCompletionSound, type ChimeProject } from '@/lib/completion-sound'
import { asArray } from '@/lib/as-array'
import type { ServerScope } from '@/kessel/server-scope'
import {
  onActivityChanged,
  onAppHello,
  onAppResync,
  onSessionActivityChanged,
  type ActivityChangedEvent,
  type ActivityDisplay,
  type ActivityRow,
  type ActivitySnapshot,
  type ActivityWorkspace,
  type SessionActivityChangedEvent,
} from '@/stores/session-events'

export type { ActivityDisplay, ActivityRow, ActivityWorkspace } from '@/stores/session-events'

/** A finished turn this client hasn't looked at (RL11). */
export interface UnseenDone {
  at: number
  agentName: string
  projectId: string | null
  workspacePath: string | null
}

/** One server's activity, as this client holds it. */
export interface ScopeActivity {
  /** The server speaks `activity_changed` (RL8): its rows are daemon rows.
   *  False until the first snapshot or frame; an older server stays false
   *  and shows its `session_activity_changed` stream (RL13). */
  supported: boolean
  instanceId: string | null
  /** The last `seq` applied. */
  seq: number
  /** `serverNow - Date.now()` at the last snapshot (for "No update in Nm"). */
  skewMs: number
  /** By session id (an older server's rows: `legacy:<agentName>`). */
  rows: ReadonlyMap<string, ActivityRow>
  /** By `workspaceKey(projectId, workspacePath)`. */
  workspaces: ReadonlyMap<string, ActivityWorkspace>
  /** By session id. */
  unseenDone: ReadonlyMap<string, UnseenDone>
}

export type ActivityViewStore = Pick<StoreApi<ScopeActivity>, 'getState' | 'getInitialState' | 'subscribe'>

// ── Pure reads (every surface goes through these) ─────────────────────────

/** RL1's rank: `waiting > working > monitoring > unverifiable > idle`. */
const RANK: Record<ActivityDisplay, number> = {
  idle: 0,
  unverifiable: 1,
  monitoring: 2,
  working: 3,
  waiting: 4,
}

export function rankDisplay(d: ActivityDisplay): number {
  return RANK[d]
}

/** The highest display in `list` (idle when empty). */
export function highestDisplay(list: Iterable<ActivityDisplay>): ActivityDisplay {
  let best: ActivityDisplay = 'idle'
  for (const d of list) if (RANK[d] > RANK[best]) best = d
  return best
}

/** Working or waiting on the human: an agent is mid-turn. Monitoring
 *  (only background work left) and unverifiable are not "busy". */
export function isBusyDisplay(d: ActivityDisplay): boolean {
  return d === 'working' || d === 'waiting'
}

/** Anything but idle: the agent may still be doing something. */
export function isLiveDisplay(d: ActivityDisplay): boolean {
  return d !== 'idle'
}

/** The rollup map key: the project id when the daemon knows it, else the
 *  path (a session under no registered project, or an older server). */
export function workspaceKey(projectId: string | null, workspacePath: string | null): string {
  return projectId ? `p:${projectId}` : `w:${workspacePath ?? ''}`
}

/** A workspace's rolled-up display, by project id first, then by path. */
export function workspaceDisplay(
  state: Pick<ScopeActivity, 'workspaces'>,
  ref: { projectId?: string | null; path?: string | null },
): ActivityDisplay {
  const byId = ref.projectId ? state.workspaces.get(workspaceKey(ref.projectId, null)) : undefined
  if (byId) return byId.display
  const byPath = ref.path ? state.workspaces.get(workspaceKey(null, ref.path)) : undefined
  return byPath?.display ?? 'idle'
}

/** The v2 `agent_name` a terminal tab spawns or attaches under (the
 *  TerminalPane rule): `attachAgentName`, else `tab-<terminalId>`. */
export function agentNameForTerminal(data: { terminalId: string; attachAgentName?: string }): string {
  return data.attachAgentName ?? `tab-${data.terminalId}`
}

/** The row whose v2 `agent_name` is `agentName`, or null. A pinned Chat's
 *  agent name is its project id (A36). */
export function rowForAgentName(state: Pick<ScopeActivity, 'rows'>, agentName: string): ActivityRow | null {
  if (!agentName) return null
  for (const row of state.rows.values()) if (row.agentName === agentName) return row
  return null
}

/** The row a terminal tab shows (RL10): by its v2 `sessionId`, which the tab
 *  only knows after reconcile (`tabs.ts`), then by its agent name. */
export function rowForTerminal(
  state: Pick<ScopeActivity, 'rows'>,
  data: { terminalId: string; sessionId?: string; attachAgentName?: string },
): ActivityRow | null {
  const bySession = data.sessionId ? state.rows.get(data.sessionId) : undefined
  return bySession ?? rowForAgentName(state, agentNameForTerminal(data))
}

/** What a terminal tab's dot shows (idle when its session has no row). */
export function terminalDisplay(
  state: Pick<ScopeActivity, 'rows'>,
  data: { terminalId: string; sessionId?: string; attachAgentName?: string },
): ActivityDisplay {
  return rowForTerminal(state, data)?.display ?? 'idle'
}

/** The highest display among the rows under `root` (an open room's Home
 *  row reads its own server's rows for its workspace). */
export function displayUnderRoot(state: Pick<ScopeActivity, 'rows'>, root: string): ActivityDisplay {
  const list: ActivityDisplay[] = []
  for (const row of state.rows.values()) {
    if (row.workspacePath && pathUnderRoot(row.workspacePath, root)) list.push(row.display)
  }
  return highestDisplay(list)
}

/** Does `projectId` have a finished turn this client hasn't seen? */
export function projectHasUnseen(state: Pick<ScopeActivity, 'unseenDone'>, projectId: string): boolean {
  for (const u of state.unseenDone.values()) if (u.projectId === projectId) return true
  return false
}

/** Does this agent name (or session) have a finished turn this client
 *  hasn't seen? */
export function agentHasUnseen(
  state: Pick<ScopeActivity, 'unseenDone'>,
  agentName: string,
  sessionId?: string | null,
): boolean {
  if (sessionId && state.unseenDone.has(sessionId)) return true
  for (const u of state.unseenDone.values()) if (u.agentName === agentName) return true
  return false
}

/** `/w/app` is under `/w`; `/w-other` is not (the daemon's own rule). */
export function pathUnderRoot(path: string, root: string): boolean {
  const norm = (p: string): string => p.replace(/\\/g, '/').replace(/\/+$/, '')
  const a = norm(path)
  const r = norm(root)
  return r.length > 0 && (a === r || a.startsWith(`${r}/`))
}

// ── Per-server stores ─────────────────────────────────────────────────────

function emptyState(): ScopeActivity {
  return {
    supported: false,
    instanceId: null,
    seq: 0,
    skewMs: 0,
    rows: new Map(),
    workspaces: new Map(),
    unseenDone: new Map(),
  }
}

const _stores = new Map<string, StoreApi<ScopeActivity>>()

function storeFor(scopeId: string): StoreApi<ScopeActivity> {
  let store = _stores.get(scopeId)
  if (!store) {
    store = createStore<ScopeActivity>(() => emptyState())
    _stores.set(scopeId, store)
  }
  return store
}

/** The activity view of `scope`'s server. Reading it subscribes to nothing
 *  on the server; `attachActivity` feeds it. One store per server: every
 *  room on that server reads the same rows. */
export function activityStore(scope: Pick<ServerScope, 'id'>): ActivityViewStore {
  return storeFor(scope.id)
}

/** React: read `scope`'s activity view (window-level surfaces pass
 *  `primaryScope()`; room components read `room.activityView`). */
export function useActivity<T>(scope: Pick<ServerScope, 'id'>, selector: (s: ScopeActivity) => T): T {
  return useStore(storeFor(scope.id), selector)
}

// ── Notifications (RL11, A36) ─────────────────────────────────────────────

/** Toasts wait this long and are cancelled if the row works again (RL11). */
export const NOTIFY_DEBOUNCE_MS = 1_500
/** The chime's "done" debounce (today's rule, A36). */
export const CHIME_DEBOUNCE_MS = 4_000
/** No unseen-done mark or chime for a turn that ends this soon after the
 *  session first worked (launch flicker; today's rule, A36). */
export const SPAWN_GRACE_MS = 5_000

/** Turn ends that never toast, mark or chime: boundaries, the human's own
 *  cancel, and process exits (RL11). */
const QUIET_REASONS = new Set([
  'session_boundary',
  'compacted',
  'interrupted',
  'prompt_dismissed',
  'agent_exited',
  'pty_exited',
])

export interface ActivityToaster {
  /** "Needs you" for a row this client isn't viewing. */
  needsYou(row: ActivityRow): void
  /** "Finished" for a row this client isn't viewing. */
  finished(row: ActivityRow): void
}

/** Who turns a scope's turn ends into toasts and chimes. The window's own
 *  server registers with toasts; a pinned room registers its workspace
 *  root (only its rows chime) and its server's project list (MS21). */
export interface ActivityNotifyHost {
  toaster: ActivityToaster | null
  /** The project list the chime's per-workspace mute reads. */
  projects(): readonly ChimeProject[]
  /** Only rows under this path notify; null = every row on the server. */
  root: string | null
}

interface Notifier {
  hosts: Set<ActivityNotifyHost>
  /** agent name → open views (pane visible + window focused). */
  viewing: Map<string, number>
  firstBusyAt: Map<string, number>
  pendingDone: Map<string, ActivityChangedEvent['turnEnded']>
  toastTimers: Map<string, ReturnType<typeof setTimeout>>
  chimeTimers: Map<string, ReturnType<typeof setTimeout>>
  waitTimers: Map<string, ReturnType<typeof setTimeout>>
}

const _notifiers = new Map<string, Notifier>()

function notifierFor(scopeId: string): Notifier {
  let n = _notifiers.get(scopeId)
  if (!n) {
    n = {
      hosts: new Set(),
      viewing: new Map(),
      firstBusyAt: new Map(),
      pendingDone: new Map(),
      toastTimers: new Map(),
      chimeTimers: new Map(),
      waitTimers: new Map(),
    }
    _notifiers.set(scopeId, n)
  }
  return n
}

/** Register who notifies for `scope`'s rows. Returns the unregister. */
export function registerActivityNotify(scope: Pick<ServerScope, 'id'>, host: ActivityNotifyHost): () => void {
  const n = notifierFor(scope.id)
  n.hosts.add(host)
  return () => {
    n.hosts.delete(host)
  }
}

function hostsFor(n: Notifier, row: ActivityRow): ActivityNotifyHost[] {
  return [...n.hosts].filter(
    (h) => h.root === null || (row.workspacePath !== null && pathUnderRoot(row.workspacePath, h.root)),
  )
}

function clearTimer(map: Map<string, ReturnType<typeof setTimeout>>, sid: string): void {
  const t = map.get(sid)
  if (t === undefined) return
  clearTimeout(t)
  map.delete(sid)
}

function isViewing(n: Notifier, row: ActivityRow): boolean {
  return (n.viewing.get(row.agentName) ?? 0) > 0
}

function dropUnseen(scopeId: string, match: (sid: string, u: UnseenDone) => boolean): void {
  const store = storeFor(scopeId)
  const cur = store.getState().unseenDone
  let next: Map<string, UnseenDone> | null = null
  for (const [sid, u] of cur) {
    if (!match(sid, u)) continue
    next ??= new Map(cur)
    next.delete(sid)
  }
  if (next) store.setState({ unseenDone: next })
}

/** This client is (or stops) looking at the session named `agentName`:
 *  its pane is visible and the window has focus. Looking clears its
 *  unseen-done mark; a turn that ends while it is looked at never marks
 *  or chimes (A36). Every `on` call is paired with one `off`. */
export function setViewing(scope: Pick<ServerScope, 'id'>, agentName: string, on: boolean): void {
  const n = notifierFor(scope.id)
  const count = n.viewing.get(agentName) ?? 0
  if (on) {
    n.viewing.set(agentName, count + 1)
    dropUnseen(scope.id, (_sid, u) => u.agentName === agentName)
  } else if (count <= 1) {
    n.viewing.delete(agentName)
  } else {
    n.viewing.set(agentName, count - 1)
  }
}

function cancelDone(n: Notifier, sid: string): void {
  clearTimer(n.toastTimers, sid)
  clearTimer(n.chimeTimers, sid)
}

function forgetSession(n: Notifier, sid: string): void {
  cancelDone(n, sid)
  clearTimer(n.waitTimers, sid)
  n.pendingDone.delete(sid)
  n.firstBusyAt.delete(sid)
}

function currentRow(scopeId: string, sid: string): ActivityRow | null {
  return storeFor(scopeId).getState().rows.get(sid) ?? null
}

function settled(row: ActivityRow | null): row is ActivityRow {
  return row !== null && (row.display === 'idle' || row.display === 'monitoring')
}

function armDone(scopeId: string, n: Notifier, row: ActivityRow): void {
  const sid = row.sessionId
  if (n.toastTimers.has(sid) || n.chimeTimers.has(sid)) return
  const hosts = hostsFor(n, row)
  if (hosts.length === 0) return
  if (hosts.some((h) => h.toaster)) {
    n.toastTimers.set(
      sid,
      setTimeout(() => {
        n.toastTimers.delete(sid)
        const cur = currentRow(scopeId, sid)
        if (!settled(cur) || isViewing(n, cur)) return
        for (const h of hostsFor(n, cur)) h.toaster?.finished(cur)
      }, NOTIFY_DEBOUNCE_MS),
    )
  }
  const first = n.firstBusyAt.get(sid)
  if (first !== undefined && Date.now() - first < SPAWN_GRACE_MS) {
    n.pendingDone.delete(sid)
    return
  }
  n.chimeTimers.set(
    sid,
    setTimeout(() => {
      n.chimeTimers.delete(sid)
      n.pendingDone.delete(sid)
      const cur = currentRow(scopeId, sid)
      if (!settled(cur) || isViewing(n, cur)) return
      const store = storeFor(scopeId)
      const unseen = new Map(store.getState().unseenDone)
      unseen.set(sid, {
        at: Date.now(),
        agentName: cur.agentName,
        projectId: cur.projectId,
        workspacePath: cur.workspacePath,
      })
      store.setState({ unseenDone: unseen })
      const host = hostsFor(n, cur)[0]
      if (host) playCompletionSound(cur.projectId, host.projects())
    }, CHIME_DEBOUNCE_MS),
  )
}

function armNeedsYou(scopeId: string, n: Notifier, row: ActivityRow): void {
  const sid = row.sessionId
  if (n.waitTimers.has(sid)) return
  if (!hostsFor(n, row).some((h) => h.toaster)) return
  n.waitTimers.set(
    sid,
    setTimeout(() => {
      n.waitTimers.delete(sid)
      const cur = currentRow(scopeId, sid)
      if (!cur || cur.display !== 'waiting' || isViewing(n, cur)) return
      for (const h of hostsFor(n, cur)) h.toaster?.needsYou(cur)
    }, NOTIFY_DEBOUNCE_MS),
  )
}

/** One applied change (a frame, or an older server's event). Snapshots
 *  never notify: they restate, they don't report a transition. */
function notifyChange(
  scopeId: string,
  prev: ActivityRow | null,
  next: ActivityRow | null,
  removed: string | null,
  turnEnded: ActivityChangedEvent['turnEnded'],
): void {
  const n = notifierFor(scopeId)
  if (removed) forgetSession(n, removed)
  if (!next) return
  const sid = next.sessionId
  const d = next.display
  if (isBusyDisplay(d)) {
    if (!n.firstBusyAt.has(sid)) n.firstBusyAt.set(sid, Date.now())
    cancelDone(n, sid)
    // The session is live again: its old "done" is no longer news.
    if (storeFor(scopeId).getState().unseenDone.has(sid)) dropUnseen(scopeId, (s) => s === sid)
  }
  if (d === 'waiting' && prev?.display !== 'waiting') armNeedsYou(scopeId, n, next)
  if (d !== 'waiting') clearTimer(n.waitTimers, sid)
  if (turnEnded) {
    const quiet =
      QUIET_REASONS.has(turnEnded.reason) || turnEnded.outcome === 'boundary' || turnEnded.outcome === 'cancelled'
    if (quiet) n.pendingDone.delete(sid)
    else n.pendingDone.set(sid, turnEnded)
  }
  if (n.pendingDone.has(sid) && settled(next)) armDone(scopeId, n, next)
}

// ── The feed (RL4) ────────────────────────────────────────────────────────

interface Engine {
  scope: ServerScope
  refs: number
  unsubs: Array<() => void>
  pulling: Promise<void> | null
  /** Frames that wait for the in-flight pull, applied after it in order. */
  buffer: ActivityChangedEvent[]
}

const _engines = new Map<string, Engine>()

function serverHasDaemonActivity(engine: Engine): boolean {
  return storeFor(engine.scope.id).getState().supported || engine.scope.serverSupports('daemon-activity')
}

function rollupMap(list: readonly ActivityWorkspace[]): Map<string, ActivityWorkspace> {
  const out = new Map<string, ActivityWorkspace>()
  for (const w of list) out.set(workspaceKey(w.projectId, w.workspacePath), w)
  return out
}

function applySnapshot(engine: Engine, snap: ActivitySnapshot): void {
  const scopeId = engine.scope.id
  const rows = new Map<string, ActivityRow>()
  for (const row of asArray<ActivityRow>(snap.rows)) rows.set(row.sessionId, row)
  const n = notifierFor(scopeId)
  for (const sid of [...n.firstBusyAt.keys(), ...n.pendingDone.keys()]) {
    if (!rows.has(sid)) forgetSession(n, sid)
  }
  storeFor(scopeId).setState({
    supported: true,
    instanceId: snap.instanceId,
    seq: snap.seq,
    skewMs: snap.serverNow - Date.now(),
    rows,
    workspaces: rollupMap(asArray<ActivityWorkspace>(snap.workspaces)),
  })
}

/** A snapshot body this client can apply (a server that answers the route
 *  with something else is treated as a failed pull). */
function isSnapshot(body: unknown): body is ActivitySnapshot {
  if (!body || typeof body !== 'object') return false
  const b = body as Partial<ActivitySnapshot>
  return typeof b.instanceId === 'string' && typeof b.seq === 'number' && Array.isArray(b.rows)
}

function applyFrame(engine: Engine, e: ActivityChangedEvent): void {
  const store = storeFor(engine.scope.id)
  const st = store.getState()
  const prev = e.row ? (st.rows.get(e.row.sessionId) ?? null) : null
  const rows = new Map(st.rows)
  if (e.removed) rows.delete(e.removed)
  if (e.row) rows.set(e.row.sessionId, e.row)
  let workspaces = st.workspaces
  if (e.workspace) {
    const next = new Map(workspaces)
    next.set(workspaceKey(e.workspace.projectId, e.workspace.workspacePath), e.workspace)
    workspaces = next
  }
  store.setState({ rows, workspaces, seq: e.seq })
  notifyChange(engine.scope.id, prev, e.row, e.removed, e.turnEnded)
}

function onFrame(engine: Engine, e: ActivityChangedEvent): void {
  const store = storeFor(engine.scope.id)
  if (!store.getState().supported) {
    // The server speaks activity_changed: drop an older-server view, if
    // any, and take the daemon's rows from the next snapshot.
    store.setState({ supported: true, rows: new Map(), workspaces: new Map() })
  }
  if (engine.pulling) {
    engine.buffer.push(e)
    return
  }
  const st = store.getState()
  if (e.instanceId !== st.instanceId) {
    // A daemon we hold no snapshot of (first frame, or a restart).
    engine.buffer.push(e)
    void pullSnapshot(engine)
    return
  }
  if (e.seq <= st.seq) return
  if (e.seq !== st.seq + 1) {
    engine.buffer.push(e)
    void pullSnapshot(engine)
    return
  }
  applyFrame(engine, e)
}

function pullSnapshot(engine: Engine): Promise<void> {
  if (engine.pulling) return engine.pulling
  const run = async (): Promise<void> => {
    let snap: ActivitySnapshot
    try {
      const body = await daemonCliGet<unknown>(engine.scope, 'activity/snapshot')
      if (!isSnapshot(body)) throw new Error('not an activity snapshot')
      snap = body
    } catch (err) {
      // The next hello (the socket reconnecting) pulls again. Frames held
      // for this pull are dropped: the next snapshot restates them.
      console.warn('[activity] snapshot pull failed:', err)
      engine.buffer = []
      return
    } finally {
      engine.pulling = null
    }
    applySnapshot(engine, snap)
    const held = engine.buffer.sort((a, b) => a.seq - b.seq)
    engine.buffer = []
    for (const frame of held) onFrame(engine, frame)
  }
  engine.pulling = run()
  return engine.pulling
}

/** RL13: an older server's `session_activity_changed`, shown as it is. */
function onLegacy(engine: Engine, e: SessionActivityChangedEvent): void {
  if (serverHasDaemonActivity(engine)) return
  const scopeId = engine.scope.id
  const store = storeFor(scopeId)
  const st = store.getState()
  const sid = `legacy:${e.agentName}`
  const display: ActivityDisplay = e.status === 'permission' ? 'waiting' : e.status
  const prev = st.rows.get(sid) ?? null
  if (prev?.display === display) return
  const now = Date.now()
  const row: ActivityRow = {
    sessionId: sid,
    agentName: e.agentName,
    projectId: null,
    workspacePath: e.workspacePath || null,
    harness: 'unknown',
    display,
    lead: {
      state: display === 'idle' ? 'idle' : display === 'waiting' ? 'waiting' : 'working',
      outcome: 'none',
      since: now,
      promptId: null,
    },
    children: { subagents: 0, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 },
    turnStartedAt: display === 'working' ? (prev?.turnStartedAt ?? now) : null,
    evidenceAt: now,
    evidenceSource: 'title',
    reason: '',
    staleSince: null,
    confirmed: true,
    rev: (prev?.rev ?? 0) + 1,
  }
  const rows = new Map(st.rows)
  rows.set(sid, row)
  // The rollup of an older server's stream: the highest display per path.
  const byPath = new Map<string, ActivityDisplay[]>()
  for (const r of rows.values()) {
    const path = r.workspacePath ?? ''
    byPath.set(path, [...(byPath.get(path) ?? []), r.display])
  }
  const workspaces = new Map<string, ActivityWorkspace>()
  for (const [path, list] of byPath) {
    const counts = { working: 0, monitoring: 0, waiting: 0, idle: 0, unverifiable: 0 }
    for (const d of list) counts[d] += 1
    workspaces.set(workspaceKey(null, path), {
      projectId: null,
      workspacePath: path,
      display: highestDisplay(list),
      counts,
      since: null,
    })
  }
  store.setState({ rows, workspaces })
  const turnEnded =
    prev && isBusyDisplay(prev.display) && display === 'idle'
      ? { outcome: 'success' as const, reason: 'turn_done', at: now }
      : null
  notifyChange(scopeId, prev, row, null, turnEnded)
}

/** Feed `scope`'s activity store from its server (refcounted: the window
 *  attaches its own server; each pinned room attaches its server). Returns
 *  the release. */
export function attachActivity(scope: ServerScope): () => void {
  let engine = _engines.get(scope.id)
  if (!engine) {
    const created: Engine = { scope, refs: 0, unsubs: [], pulling: null, buffer: [] }
    created.unsubs.push(
      onActivityChanged(scope, (e) => onFrame(created, e)),
      onAppHello(scope, () => {
        if (serverHasDaemonActivity(created)) void pullSnapshot(created)
      }),
      onAppResync(scope, () => {
        if (serverHasDaemonActivity(created)) void pullSnapshot(created)
      }),
      onSessionActivityChanged(scope, (e) => onLegacy(created, e)),
    )
    _engines.set(scope.id, created)
    engine = created
    // The socket may already be open (no hello is coming): pull now.
    if (serverHasDaemonActivity(created)) void pullSnapshot(created)
  }
  engine.refs += 1
  let released = false
  const owned = engine
  return () => {
    if (released) return
    released = true
    owned.refs -= 1
    if (owned.refs > 0) return
    for (const u of owned.unsubs) u()
    _engines.delete(scope.id)
  }
}

/** Forget everything held for `scope` (a server switch: the window's
 *  scope now points at another daemon). The feed stays attached; the next
 *  hello pulls the new server's snapshot. */
export function resetActivity(scope: Pick<ServerScope, 'id'>): void {
  const n = _notifiers.get(scope.id)
  if (n) {
    for (const sid of new Set([...n.toastTimers.keys(), ...n.chimeTimers.keys(), ...n.waitTimers.keys()])) {
      forgetSession(n, sid)
    }
    n.firstBusyAt.clear()
    n.pendingDone.clear()
  }
  const engine = _engines.get(scope.id)
  if (engine) engine.buffer = []
  storeFor(scope.id).setState(emptyState())
}

/** Test seam: set what `scope`'s activity view holds (rows keyed by
 *  session id; rollups by `workspaceKey`). */
export function __seedActivityForTests(
  scope: Pick<ServerScope, 'id'>,
  seed: { rows?: ActivityRow[]; workspaces?: ActivityWorkspace[]; unseenDone?: Map<string, UnseenDone>; supported?: boolean },
): void {
  const patch: Partial<ScopeActivity> = {}
  if (seed.rows) patch.rows = new Map(seed.rows.map((r) => [r.sessionId, r]))
  if (seed.workspaces) patch.workspaces = rollupMap(seed.workspaces)
  if (seed.unseenDone) patch.unseenDone = seed.unseenDone
  if (seed.supported !== undefined) patch.supported = seed.supported
  storeFor(scope.id).setState(patch)
}

/** Test seam: drop every store, feed and timer. */
export function __resetActivityForTests(): void {
  for (const engine of _engines.values()) for (const u of engine.unsubs) u()
  _engines.clear()
  for (const n of _notifiers.values()) {
    for (const t of [...n.toastTimers.values(), ...n.chimeTimers.values(), ...n.waitTimers.values()]) clearTimeout(t)
  }
  _notifiers.clear()
  for (const store of _stores.values()) store.setState(emptyState())
}
