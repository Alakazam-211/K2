// prd-daemon-activity-and-thread-working-v1 S5 — the client's copy of the
// daemon's activity rows (T-S5a, T-S5c, T-S5d, T-S5e old server, T-S5f).
//
// The feed is driven through the real store with the app bus and the
// snapshot route faked: frames go in through the handlers the store
// registered, snapshots come back from a queued `daemonCliGet`. Fail loud —
// no skips, no `??` defaults in assertions.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  ActivityChangedEvent,
  ActivityDisplay,
  ActivityRow,
  ActivitySnapshot,
  ActivityWorkspace,
  AgentStatusChangedEvent,
  HelloEvent,
  SessionActivityChangedEvent,
  SessionAddedEvent,
  SessionRemovedEvent,
} from '@/stores/session-events'

type Handlers = {
  activity: Array<(e: ActivityChangedEvent) => void>
  hello: Array<(e: HelloEvent) => void>
  resync: Array<() => void>
  legacy: Array<(e: SessionActivityChangedEvent) => void>
  agentStatus: Array<(e: AgentStatusChangedEvent) => void>
  sessionAdded: Array<(e: SessionAddedEvent) => void>
  sessionRemoved: Array<(e: SessionRemovedEvent) => void>
}

/** One app bus per server (scope id), like the real registry. */
const buses = new Map<string, Handlers>()
function busOf(scopeId: string): Handlers {
  let b = buses.get(scopeId)
  if (!b) {
    b = { activity: [], hello: [], resync: [], legacy: [], agentStatus: [], sessionAdded: [], sessionRemoved: [] }
    buses.set(scopeId, b)
  }
  return b
}

function add<T>(list: T[], fn: T): () => void {
  list.push(fn)
  return () => {
    const i = list.indexOf(fn)
    if (i >= 0) list.splice(i, 1)
  }
}

type ScopeArg = { id: string }
vi.mock('@/stores/session-events', () => ({
  onActivityChanged: (s: ScopeArg, fn: (e: ActivityChangedEvent) => void) => add(busOf(s.id).activity, fn),
  onAppHello: (s: ScopeArg, fn: (e: HelloEvent) => void) => add(busOf(s.id).hello, fn),
  onAppResync: (s: ScopeArg, fn: () => void) => add(busOf(s.id).resync, fn),
  onSessionActivityChanged: (s: ScopeArg, fn: (e: SessionActivityChangedEvent) => void) => add(busOf(s.id).legacy, fn),
  onAgentStatusChanged: (s: ScopeArg, fn: (e: AgentStatusChangedEvent) => void) => add(busOf(s.id).agentStatus, fn),
  onSessionAddedApp: (s: ScopeArg, fn: (e: SessionAddedEvent) => void) => add(busOf(s.id).sessionAdded, fn),
  onSessionRemovedApp: (s: ScopeArg, fn: (e: SessionRemovedEvent) => void) => add(busOf(s.id).sessionRemoved, fn),
}))

/** Snapshot replies per server, in order; a pull with none queued fails the
 *  pull (and the test, through the `afterEach` / assertions). */
const snapshotQueues = new Map<string, ActivitySnapshot[]>()
function snapshotsOf(scopeId: string): ActivitySnapshot[] {
  let q = snapshotQueues.get(scopeId)
  if (!q) {
    q = []
    snapshotQueues.set(scopeId, q)
  }
  return q
}
/** `GET /cli/agents/running` per server (an older server's agent ↔ session ids). */
const running = new Map<string, Array<{ terminalId: string; agentName: string; cwd: string }>>()
const daemonCliGet = vi.fn(async (scope: ScopeArg, route: string): Promise<unknown> => {
  if (route === 'agents/running') return running.get(scope.id) ?? []
  if (route !== 'activity/snapshot') throw new Error(`unexpected GET ${route}`)
  const next = snapshotsOf(scope.id).shift()
  if (!next) throw new Error('a snapshot was pulled but the test queued none')
  return next
})
const daemonCliPost = vi.fn(async () => ({}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (scope: ScopeArg, route: string) => daemonCliGet(scope, route),
  daemonCliPost: () => daemonCliPost(),
}))
vi.mock('@/lib/completion-sound', () => ({ playCompletionSound: vi.fn() }))

import { playCompletionSound } from '@/lib/completion-sound'
import {
  CHIME_DEBOUNCE_MS,
  NOTIFY_DEBOUNCE_MS,
  SPAWN_GRACE_MS,
  __resetActivityForTests,
  activityStore,
  agentHasUnseen,
  attachActivity,
  countsUnderRoot,
  displayUnderRoot,
  forgetLegacyScreen,
  isLegacyActivityServer,
  noteLegacyScreen,
  projectHasUnseen,
  registerActivityNotify,
  resetActivity,
  rowForAgentName,
  rowForTerminal,
  setViewing,
  terminalDisplay,
  workspaceCounts,
  workspaceDisplay,
  type ActivityToaster,
} from './activity'
import { LEGACY_SCREEN_GRACE_MS } from './activity-legacy'
import type { ServerScope } from '@/kessel/server-scope'

let supports = true
const SCOPE = { id: 'host:test', serverSupports: () => supports } as unknown as ServerScope
const bus = busOf(SCOPE.id)
const snapshots = snapshotsOf(SCOPE.id)
const INSTANCE = 'inst-1'

function row(sessionId: string, display: ActivityDisplay, over: Partial<ActivityRow> = {}): ActivityRow {
  return {
    sessionId,
    agentName: `tab-${sessionId}`,
    projectId: 'p1',
    workspacePath: '/srv/ws',
    harness: 'claude',
    display,
    lead: { state: display === 'working' ? 'working' : display === 'waiting' ? 'waiting' : 'idle', outcome: 'none', since: 0, promptId: null },
    children: { subagents: 0, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 },
    turnStartedAt: null,
    evidenceAt: 1,
    evidenceSource: 'hook',
    reason: 'turn_running',
    staleSince: null,
    confirmed: true,
    rev: 1,
    ...over,
  }
}

function rollup(display: ActivityDisplay, projectId = 'p1', workspacePath = '/srv/ws'): ActivityWorkspace {
  const counts = { working: 0, monitoring: 0, waiting: 0, unverifiable: 0, idle: 0 }
  counts[display] = 1
  return { projectId, workspacePath, display, counts, since: null }
}

function snap(seq: number, rows: ActivityRow[], instanceId = INSTANCE): ActivitySnapshot {
  return {
    instanceId,
    seq,
    serverNow: Date.now(),
    staleAfterSecs: 1800,
    rows,
    workspaces: rows.length > 0 ? [rollup(rows[0].display)] : [],
  }
}

function frame(seq: number, r: ActivityRow | null, over: Partial<ActivityChangedEvent> = {}): ActivityChangedEvent {
  return {
    kind: 'activity_changed',
    seq,
    instanceId: INSTANCE,
    row: r,
    removed: null,
    turnEnded: null,
    workspace: r ? rollup(r.display) : null,
    ...over,
  }
}

function emit(e: ActivityChangedEvent): void {
  if (bus.activity.length !== 1) throw new Error(`expected one activity handler, got ${bus.activity.length}`)
  bus.activity[0](e)
}

function hello(instanceId = INSTANCE): void {
  for (const fn of bus.hello) fn({ kind: 'hello', workspace_path: '', subscriber_id: 1, instance_id: instanceId })
}

/** Let the pulled snapshot's promise chain settle. */
async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve()
}

function state() {
  return activityStore(SCOPE).getState()
}

function mustRow(sid: string): ActivityRow {
  const r = state().rows.get(sid)
  if (!r) throw new Error(`no row ${sid}`)
  return r
}

beforeEach(() => {
  __resetActivityForTests()
  daemonCliGet.mockClear()
  daemonCliPost.mockClear()
  vi.mocked(playCompletionSound).mockClear()
  for (const q of snapshotQueues.values()) q.length = 0
  running.clear()
  supports = true
})

afterEach(() => {
  __resetActivityForTests()
  vi.useRealTimers()
  for (const [id, q] of snapshotQueues) {
    if (q.length !== 0) throw new Error(`${q.length} queued snapshot(s) for ${id} were never pulled`)
  }
  for (const [id, b] of buses) {
    const left = Object.entries(b).filter(([, list]) => list.length > 0)
    if (left.length > 0) throw new Error(`${id}: handlers left after reset: ${left.map(([k]) => k).join(', ')}`)
  }
})

describe('the feed (T-S5a, RL4)', () => {
  it('pulls once on attach, and the snapshot replaces the view', async () => {
    snapshots.push(snap(10, [row('s1', 'working')]))
    attachActivity(SCOPE)
    await settle()
    expect(daemonCliGet).toHaveBeenCalledTimes(1)
    expect(state().supported).toBe(true)
    expect(state().seq).toBe(10)
    expect(state().instanceId).toBe(INSTANCE)
    expect(mustRow('s1').display).toBe('working')
    expect(workspaceDisplay(state(), { projectId: 'p1' })).toBe('working')

    // A hello (socket reopened: a reconnect, a 4008 lag close, a restart)
    // pulls again, and the new snapshot REPLACES — s1 is gone.
    snapshots.push(snap(3, [row('s2', 'waiting')], 'inst-2'))
    hello('inst-2')
    await settle()
    expect(daemonCliGet).toHaveBeenCalledTimes(2)
    expect([...state().rows.keys()]).toEqual(['s2'])
    expect(state().instanceId).toBe('inst-2')
    expect(state().seq).toBe(3)
  })

  it('applies frames in seq order and drops older ones', async () => {
    snapshots.push(snap(5, [row('s1', 'idle')]))
    attachActivity(SCOPE)
    await settle()
    emit(frame(6, row('s1', 'working', { rev: 2 })))
    expect(mustRow('s1').display).toBe('working')
    expect(state().seq).toBe(6)
    // An older (or repeated) seq never rolls the row back.
    emit(frame(6, row('s1', 'idle', { rev: 1 })))
    emit(frame(4, row('s1', 'idle', { rev: 1 })))
    expect(mustRow('s1').display).toBe('working')
    expect(state().seq).toBe(6)
    emit(frame(7, row('s1', 'monitoring', { rev: 3 })))
    expect(mustRow('s1').display).toBe('monitoring')
    expect(daemonCliGet).toHaveBeenCalledTimes(1)
  })

  it('a seq gap pulls exactly one snapshot; frames held meanwhile apply after it in order', async () => {
    snapshots.push(snap(5, [row('s1', 'idle')]))
    attachActivity(SCOPE)
    await settle()
    // seq 7 arrives without 6: gap → one pull.
    snapshots.push(snap(7, [row('s1', 'working', { rev: 3 })]))
    emit(frame(7, row('s1', 'working', { rev: 3 })))
    // While the pull is in flight more frames arrive, out of order.
    emit(frame(9, row('s1', 'waiting', { rev: 5 })))
    emit(frame(8, row('s1', 'idle', { rev: 4 })))
    await settle()
    expect(daemonCliGet).toHaveBeenCalledTimes(2)
    expect(state().seq).toBe(9)
    expect(mustRow('s1').display).toBe('waiting')
  })

  it('a frame from another daemon instance pulls one snapshot', async () => {
    snapshots.push(snap(5, [row('s1', 'idle')]))
    attachActivity(SCOPE)
    await settle()
    snapshots.push(snap(1, [row('s9', 'working')], 'inst-2'))
    emit(frame(1, row('s9', 'working'), { instanceId: 'inst-2' }))
    await settle()
    expect(daemonCliGet).toHaveBeenCalledTimes(2)
    expect([...state().rows.keys()]).toEqual(['s9'])
    expect(state().instanceId).toBe('inst-2')
  })

  it('a resync control frame pulls one snapshot', async () => {
    snapshots.push(snap(5, []))
    attachActivity(SCOPE)
    await settle()
    snapshots.push(snap(40, [row('s1', 'working')]))
    for (const fn of bus.resync) fn()
    await settle()
    expect(daemonCliGet).toHaveBeenCalledTimes(2)
    expect(state().seq).toBe(40)
  })

  it('a removal drops the row and updates the rollup', async () => {
    snapshots.push(snap(1, [row('s1', 'working')]))
    attachActivity(SCOPE)
    await settle()
    emit(frame(2, null, { removed: 's1', workspace: rollup('idle') }))
    expect(state().rows.has('s1')).toBe(false)
    expect(workspaceDisplay(state(), { projectId: 'p1' })).toBe('idle')
  })

  it('one feed per server: a second attach shares it, and the last release unsubscribes', async () => {
    snapshots.push(snap(1, []))
    const a = attachActivity(SCOPE)
    const b = attachActivity(SCOPE)
    await settle()
    expect(daemonCliGet).toHaveBeenCalledTimes(1)
    expect(bus.activity.length).toBe(1)
    a()
    expect(bus.activity.length).toBe(1)
    b()
    expect(bus.activity.length).toBe(0)
  })

  it('a server switch reset forgets the old rows; the next hello pulls the new server', async () => {
    snapshots.push(snap(9, [row('s1', 'working')]))
    attachActivity(SCOPE)
    await settle()
    resetActivity(SCOPE)
    expect(state().rows.size).toBe(0)
    expect(state().instanceId).toBe(null)
    snapshots.push(snap(2, [row('r1', 'idle')], 'inst-remote'))
    hello('inst-remote')
    await settle()
    expect([...state().rows.keys()]).toEqual(['r1'])
  })
})

// ── An older server: the legacy adapter (Q5, RL13, T-S5e) ─────────────────
//
// The frame shapes below are what a 0.44.4 daemon (`v0.44.4`, built and run
// headless on a temp HOME with a shim agent) sent a client's app socket for
// one shim turn: `session_added`, then `session_activity_changed` working →
// idle from its title/bell observer; `GET /cli/activity/snapshot` → 404;
// `GET /cli/agents/running` → `[{terminalId, agentName, cwd, …}]`. Paths are
// synthesized.

const OLD = { id: 'host:old', serverSupports: () => false } as unknown as ServerScope
const OLD_PROJECTS = [
  { id: 'p-old', path: '/srv/old' },
  { id: 'p-nested', path: '/srv/old/nested' },
]

function oldState() {
  return activityStore(OLD).getState()
}

function observerOn(scope: ServerScope, agentName: string, status: SessionActivityChangedEvent['status'], workspacePath = '/srv/old'): void {
  const b = busOf(scope.id)
  if (b.legacy.length !== 1) throw new Error(`expected one observer handler on ${scope.id}, got ${b.legacy.length}`)
  b.legacy[0]({
    kind: 'session_activity_changed',
    workspacePath,
    agentName,
    paneGroupId: agentName.startsWith('tab-') ? agentName.slice(4) : null,
    status,
  })
}

function hookOn(scope: ServerScope, paneId: string, status: AgentStatusChangedEvent['status'], workspacePath = '/srv/old'): void {
  const b = busOf(scope.id)
  if (b.agentStatus.length !== 1) throw new Error(`expected one hook handler on ${scope.id}, got ${b.agentStatus.length}`)
  b.agentStatus[0]({ kind: 'agent_status_changed', paneId, tabId: paneId, status, workspacePath })
}

function sessionAddedOn(scope: ServerScope, agentName: string, sessionId: string, workspacePath = '/srv/old'): void {
  const b = busOf(scope.id)
  if (b.sessionAdded.length !== 1) throw new Error(`expected one session_added handler on ${scope.id}`)
  b.sessionAdded[0]({
    kind: 'session_added',
    workspace_path: workspacePath,
    pane_group_id: agentName.startsWith('tab-') ? agentName.slice(4) : null,
    agent_name: agentName,
    command: 'claude',
    args: [],
    session_id: sessionId,
    isV2: true,
  })
}

function helloOn(scope: ServerScope, instanceId: string): void {
  for (const fn of busOf(scope.id).hello) fn({ kind: 'hello', workspace_path: '', subscriber_id: 1, instance_id: instanceId })
}

function getRoutes(): string[] {
  return daemonCliGet.mock.calls.map((c) => `${c[0].id} ${c[1]}`)
}

async function attachOld(toaster: ActivityToaster | null = null): Promise<void> {
  attachActivity(OLD)
  registerActivityNotify(OLD, { toaster, projects: () => OLD_PROJECTS, root: null })
  await settle()
}

describe('an older server: legacy events drive the dots (Q5, RL13, T-S5e)', () => {
  it('the observer stream lights a tab, the Sidebar, the Active bar, Home and a room, with no snapshot pull', async () => {
    running.set(OLD.id, [{ terminalId: 'sid-1', agentName: 'tab-t1', cwd: '/srv/old' }])
    await attachOld()
    expect(isLegacyActivityServer(OLD)).toBe(true)
    // Seeded agent ↔ session ids; never asked for a snapshot it hasn't got.
    expect(getRoutes()).toEqual(['host:old agents/running'])

    sessionAddedOn(OLD, 'p-old', 'sid-chat')
    observerOn(OLD, 'tab-t1', 'working')
    expect(oldState().supported).toBe(false)
    // A tab: before reconcile by its agent name, after by its session id.
    expect(terminalDisplay(oldState(), { terminalId: 't1' })).toBe('working')
    expect(terminalDisplay(oldState(), { terminalId: 't1', sessionId: 'sid-1' })).toBe('working')
    // The Sidebar's spinner reads by project id alone (Sidebar.tsx AgentSpinner).
    expect(workspaceDisplay(oldState(), { projectId: 'p-old' })).toBe('working')
    // The Active bar reads by id, then path.
    expect(workspaceDisplay(oldState(), { projectId: 'p-old', path: '/srv/old' })).toBe('working')
    // Home's open-room row and the room's tab strip read the same rows.
    expect(displayUnderRoot(oldState(), '/srv/old')).toBe('working')
    expect(mustOldRow('sid-1').projectId).toBe('p-old')

    // The pinned Chat (agent name = project id) on the TabBar.
    observerOn(OLD, 'p-old', 'permission')
    expect(rowForAgentName(oldState(), 'p-old')?.display).toBe('waiting')
    expect(workspaceDisplay(oldState(), { projectId: 'p-old' })).toBe('waiting')

    // A session in a nested workspace belongs to the deepest project.
    observerOn(OLD, 'tab-n1', 'working', '/srv/old/nested/app')
    expect(rowForAgentName(oldState(), 'tab-n1')?.projectId).toBe('p-nested')
    expect(workspaceDisplay(oldState(), { projectId: 'p-nested' })).toBe('working')

    observerOn(OLD, 'tab-t1', 'idle')
    observerOn(OLD, 'p-old', 'idle')
    observerOn(OLD, 'tab-n1', 'idle')
    expect(terminalDisplay(oldState(), { terminalId: 't1', sessionId: 'sid-1' })).toBe('idle')
    expect(workspaceDisplay(oldState(), { projectId: 'p-old' })).toBe('idle')
    expect(displayUnderRoot(oldState(), '/srv/old')).toBe('idle')
    expect(getRoutes()).toEqual(['host:old agents/running'])
  })

  it('hooks: a permission wins, a start alone lights its session, and one session is one row', async () => {
    running.set(OLD.id, [{ terminalId: 'sid-1', agentName: 'tab-t1', cwd: '/srv/old' }])
    await attachOld()

    hookOn(OLD, 'sid-1', 'start')
    expect([...oldState().rows.keys()]).toEqual(['sid-1'])
    expect(terminalDisplay(oldState(), { terminalId: 't1', sessionId: 'sid-1' })).toBe('working')
    // Once the observer has spoken for the session it decides working/idle.
    observerOn(OLD, 'tab-t1', 'idle')
    expect([...oldState().rows.keys()]).toEqual(['sid-1'])
    expect(terminalDisplay(oldState(), { terminalId: 't1' })).toBe('idle')
    // …but a hook permission wins over it (claude's "needs you").
    observerOn(OLD, 'tab-t1', 'working')
    hookOn(OLD, 'sid-1', 'permission')
    expect(terminalDisplay(oldState(), { terminalId: 't1' })).toBe('waiting')
    expect(workspaceDisplay(oldState(), { projectId: 'p-old' })).toBe('waiting')
    hookOn(OLD, 'sid-1', 'start')
    expect(terminalDisplay(oldState(), { terminalId: 't1' })).toBe('working')

    // A hook for a session this client hasn't matched to an agent name: its
    // own row, found by the tab's session id.
    hookOn(OLD, 'sid-9', 'start')
    expect(terminalDisplay(oldState(), { terminalId: 't9', sessionId: 'sid-9' })).toBe('working')
    // `session_added` matches it; the observer then decides for it.
    sessionAddedOn(OLD, 'tab-t9', 'sid-9')
    observerOn(OLD, 'tab-t9', 'idle')
    expect(terminalDisplay(oldState(), { terminalId: 't9' })).toBe('idle')
    expect([...oldState().rows.keys()].sort()).toEqual(['sid-1', 'sid-9'])
    // `session_removed` forgets it.
    for (const fn of busOf(OLD.id).sessionRemoved) {
      fn({ kind: 'session_removed', workspace_path: '/srv/old', pane_group_id: 't9', agent_name: 'tab-t9' })
    }
    expect([...oldState().rows.keys()]).toEqual(['sid-1'])
  })

  it("a pane's busy footer lights a harness with no title and no hook, and decays", async () => {
    vi.useFakeTimers()
    attachActivity(OLD)
    registerActivityNotify(OLD, { toaster: null, projects: () => OLD_PROJECTS, root: null })
    await vi.advanceTimersByTimeAsync(0)

    noteLegacyScreen(OLD, 'tab-h1', ['', 'Hermes ▸ msg=interrupt · /queue · /bg'], '/srv/old')
    expect(terminalDisplay(oldState(), { terminalId: 'h1' })).toBe('working')
    expect(workspaceDisplay(oldState(), { projectId: 'p-old' })).toBe('working')
    // Each frame that still shows it renews it.
    await vi.advanceTimersByTimeAsync(LEGACY_SCREEN_GRACE_MS - 100)
    noteLegacyScreen(OLD, 'tab-h1', ['msg=interrupt'], '/srv/old')
    await vi.advanceTimersByTimeAsync(LEGACY_SCREEN_GRACE_MS - 100)
    expect(terminalDisplay(oldState(), { terminalId: 'h1' })).toBe('working')
    await vi.advanceTimersByTimeAsync(200)
    expect(terminalDisplay(oldState(), { terminalId: 'h1' })).toBe('idle')

    // A screen with no footer lights nothing.
    noteLegacyScreen(OLD, 'tab-h2', ['$ ls', 'README.md'], '/srv/old')
    expect(terminalDisplay(oldState(), { terminalId: 'h2' })).toBe('idle')
    // An unmounted pane stops counting at once.
    noteLegacyScreen(OLD, 'tab-h3', ['esc to cancel'], '/srv/old')
    expect(terminalDisplay(oldState(), { terminalId: 'h3' })).toBe('working')
    forgetLegacyScreen(OLD, 'tab-h3')
    expect(terminalDisplay(oldState(), { terminalId: 'h3' })).toBe('idle')
    // Once the observer has spoken for a session, the footer doesn't override it.
    observerOn(OLD, 'tab-h1', 'idle')
    noteLegacyScreen(OLD, 'tab-h1', ['esc to interrupt'], '/srv/old')
    expect(terminalDisplay(oldState(), { terminalId: 'h1' })).toBe('idle')
  })

  it('a busy → idle change is a finished turn: toast, unseen done on its project, chime', async () => {
    vi.useFakeTimers()
    const toaster: ActivityToaster = { needsYou: vi.fn(), finished: vi.fn() }
    running.set(OLD.id, [{ terminalId: 'sid-1', agentName: 'tab-t1', cwd: '/srv/old' }])
    attachActivity(OLD)
    registerActivityNotify(OLD, { toaster, projects: () => OLD_PROJECTS, root: null })
    await vi.advanceTimersByTimeAsync(0)
    observerOn(OLD, 'tab-t1', 'working')
    await vi.advanceTimersByTimeAsync(SPAWN_GRACE_MS)
    observerOn(OLD, 'tab-t1', 'idle')
    await vi.advanceTimersByTimeAsync(CHIME_DEBOUNCE_MS)
    expect(toaster.finished).toHaveBeenCalledTimes(1)
    expect(projectHasUnseen(oldState(), 'p-old')).toBe(true)
    expect(agentHasUnseen(oldState(), 'tab-t1', 'sid-1')).toBe(true)
    expect(playCompletionSound).toHaveBeenCalledTimes(1)
    expect(vi.mocked(playCompletionSound).mock.calls[0][0]).toBe('p-old')
    // Needs you, from a hook permission.
    hookOn(OLD, 'sid-1', 'permission')
    await vi.advanceTimersByTimeAsync(NOTIFY_DEBOUNCE_MS)
    expect(toaster.needsYou).toHaveBeenCalledTimes(1)
  })

  it('a server switch reset drops the legacy view; the next events light it again', async () => {
    await attachOld()
    observerOn(OLD, 'tab-t1', 'working')
    resetActivity(OLD)
    expect(oldState().rows.size).toBe(0)
    observerOn(OLD, 'tab-t2', 'working')
    expect([...oldState().rows.keys()]).toEqual(['legacy:tab-t2'])
  })

  it('features read as absent: a hello re-seeds agent ids and never pulls a snapshot', async () => {
    await attachOld()
    helloOn(OLD, 'inst-old')
    await settle()
    expect(getRoutes()).toEqual(['host:old agents/running', 'host:old agents/running'])
  })
})

describe('a server with daemon activity ignores every legacy event (RL13)', () => {
  it('daemon rows drive the dots; observer, hook, session and footer events change nothing', async () => {
    snapshots.push(snap(1, [row('s1', 'working', { agentName: 'tab-t1' })]))
    attachActivity(SCOPE)
    await settle()
    expect(isLegacyActivityServer(SCOPE)).toBe(false)
    observerOn(SCOPE, 'tab-t1', 'idle', '/srv/ws')
    observerOn(SCOPE, 'tab-x', 'working', '/srv/ws')
    hookOn(SCOPE, 's1', 'permission', '/srv/ws')
    hookOn(SCOPE, 's2', 'start', '/srv/ws')
    sessionAddedOn(SCOPE, 'tab-y', 's3', '/srv/ws')
    noteLegacyScreen(SCOPE, 'tab-t1', ['esc to interrupt'], '/srv/ws')
    expect([...state().rows.keys()]).toEqual(['s1'])
    expect(terminalDisplay(state(), { terminalId: 't1', sessionId: 's1' })).toBe('working')
    expect(workspaceDisplay(state(), { projectId: 'p1' })).toBe('working')
    emit(frame(2, row('s1', 'idle', { agentName: 'tab-t1' })))
    expect(terminalDisplay(state(), { terminalId: 't1', sessionId: 's1' })).toBe('idle')
    expect(getRoutes()).toEqual(['host:test activity/snapshot'])
  })
})

describe('a window on a new server and a room on an old one (RL13, per server)', () => {
  it('each server is fed its own way, and neither leaks into the other', async () => {
    snapshots.push(snap(1, [row('s1', 'idle', { agentName: 'tab-new' })]))
    attachActivity(SCOPE)
    registerActivityNotify(SCOPE, { toaster: null, projects: () => [{ id: 'p1', path: '/srv/ws' }], root: null })
    running.set(OLD.id, [{ terminalId: 'sid-old', agentName: 'tab-old', cwd: '/srv/old' }])
    attachActivity(OLD)
    registerActivityNotify(OLD, { toaster: null, projects: () => OLD_PROJECTS, root: '/srv/old' })
    await settle()
    expect(getRoutes().sort()).toEqual(['host:old agents/running', 'host:test activity/snapshot'])

    // The room's old server: its observer lights its rows only.
    observerOn(OLD, 'tab-old', 'working')
    expect(terminalDisplay(oldState(), { terminalId: 'old', sessionId: 'sid-old' })).toBe('working')
    expect(displayUnderRoot(oldState(), '/srv/old')).toBe('working')
    expect(terminalDisplay(state(), { terminalId: 'old', sessionId: 'sid-old' })).toBe('idle')
    // The window's new server: its compat stream is ignored, its frames rule.
    observerOn(SCOPE, 'tab-new', 'working', '/srv/ws')
    expect(terminalDisplay(state(), { terminalId: 'new', sessionId: 's1' })).toBe('idle')
    emit(frame(2, row('s1', 'working', { agentName: 'tab-new' })))
    expect(terminalDisplay(state(), { terminalId: 'new', sessionId: 's1' })).toBe('working')
    expect(workspaceDisplay(state(), { projectId: 'p1' })).toBe('working')
    // …and the old server's view is untouched by the new one's frame.
    expect([...oldState().rows.keys()]).toEqual(['sid-old'])
    expect(oldState().supported).toBe(false)
    expect(state().supported).toBe(true)
  })
})

describe("before a server's features are read (RL13)", () => {
  it('its compat stream keeps the dots lit; once it reports daemon-activity, the next hello pulls and daemon rows take over', async () => {
    let known = false
    const LATE = { id: 'host:late', serverSupports: (f: string) => known && f === 'daemon-activity' } as unknown as ServerScope
    const late = () => activityStore(LATE).getState()
    running.set(LATE.id, [{ terminalId: 'sid-1', agentName: 'tab-t1', cwd: '/srv/late' }])
    attachActivity(LATE)
    await settle()
    expect(getRoutes()).toEqual(['host:late agents/running'])
    // Not empty meanwhile: a new server's compat stream is shown.
    observerOn(LATE, 'tab-t1', 'working', '/srv/late')
    expect(terminalDisplay(late(), { terminalId: 't1', sessionId: 'sid-1' })).toBe('working')

    // Its /boot-status is read: it has daemon activity. The next hello
    // (or frame) pulls, and the snapshot replaces the legacy view.
    known = true
    expect(isLegacyActivityServer(LATE)).toBe(false)
    snapshotsOf(LATE.id).push(snap(4, [row('sid-1', 'monitoring', { agentName: 'tab-t1', workspacePath: '/srv/late' })]))
    helloOn(LATE, INSTANCE)
    await settle()
    expect(late().supported).toBe(true)
    expect([...late().rows.keys()]).toEqual(['sid-1'])
    expect(terminalDisplay(late(), { terminalId: 't1', sessionId: 'sid-1' })).toBe('monitoring')
    // From now on its compat stream is ignored.
    observerOn(LATE, 'tab-t1', 'idle', '/srv/late')
    expect(terminalDisplay(late(), { terminalId: 't1', sessionId: 'sid-1' })).toBe('monitoring')
  })

  it('the first activity_changed frame proves daemon rows even before its features are read', async () => {
    supports = false
    attachActivity(SCOPE)
    await settle()
    observerOn(SCOPE, 'tab-x', 'working', '/srv/ws')
    expect([...state().rows.keys()]).toEqual(['legacy:tab-x'])
    snapshots.push(snap(4, [row('s1', 'idle')]))
    emit(frame(4, row('s1', 'idle')))
    await settle()
    expect(state().supported).toBe(true)
    expect([...state().rows.keys()]).toEqual(['s1'])
    observerOn(SCOPE, 'tab-x', 'working', '/srv/ws')
    expect([...state().rows.keys()]).toEqual(['s1'])
  })
})

function mustOldRow(sid: string): ActivityRow {
  const r = oldState().rows.get(sid)
  if (!r) throw new Error(`no row ${sid} on the old server`)
  return r
}

describe('counts (0.45.2): live subagents, tools and commands per workspace', () => {
  const c = (subagents: number, tools: number, commands: number) => ({ subagents, tools, commands })
  const rows = (list: ActivityRow[]) => ({ rows: new Map(list.map((r) => [r.sessionId, r])) })

  it('sums the busy sessions of a project; idle sessions and rows without counts add nothing', () => {
    const st = rows([
      row('a', 'working', { counts: c(2, 10, 4) }),
      row('b', 'waiting', { counts: c(0, 3, 1) }),
      row('c', 'monitoring', { counts: c(1, 0, 0) }),
      row('d', 'idle', { counts: c(0, 99, 99) }),
      row('e', 'working'),
      row('f', 'working', { projectId: 'p2', counts: c(5, 5, 5) }),
    ])
    expect(workspaceCounts(st, { projectId: 'p1', path: '/srv/ws' })).toEqual(c(3, 13, 5))
    expect(workspaceCounts(st, { projectId: 'p2' })).toEqual(c(5, 5, 5))
    expect(workspaceCounts(st, { projectId: 'none' })).toEqual(c(0, 0, 0))
  })

  it('falls back to rows with no project at exactly the path, like the rollup', () => {
    const st = rows([
      row('a', 'working', { projectId: null, workspacePath: '/srv/loose', counts: c(0, 4, 2) }),
      row('b', 'working', { projectId: null, workspacePath: '/srv/loose/sub', counts: c(0, 7, 7) }),
    ])
    expect(workspaceCounts(st, { projectId: 'unknown', path: '/srv/loose' })).toEqual(c(0, 4, 2))
    expect(workspaceCounts(st, { path: '/srv/loose' })).toEqual(c(0, 4, 2))
    expect(workspaceCounts(st, {})).toEqual(c(0, 0, 0))
  })

  it('an open room sums the busy rows under its root', () => {
    const st = rows([
      row('a', 'working', { workspacePath: '/srv/ws', counts: c(1, 2, 1) }),
      row('b', 'working', { workspacePath: '/srv/ws/wt', counts: c(0, 3, 0) }),
      row('c', 'working', { workspacePath: '/srv/ws-other', counts: c(9, 9, 9) }),
      row('d', 'idle', { workspacePath: '/srv/ws', counts: c(9, 9, 9) }),
    ])
    expect(countsUnderRoot(st, '/srv/ws')).toEqual(c(1, 5, 1))
  })
})

describe('surface mappings (T-S5c, RL10)', () => {
  it('a tab maps to its row by sessionId, then by its agent name', async () => {
    snapshots.push(
      snap(1, [
        row('sid-a', 'working', { agentName: 'tab-term-a' }),
        row('sid-b', 'monitoring', { agentName: 'cortana-hb' }),
      ]),
    )
    attachActivity(SCOPE)
    await settle()
    expect(rowForTerminal(state(), { terminalId: 'term-x', sessionId: 'sid-a' })?.sessionId).toBe('sid-a')
    // Before reconcile the tab has no sessionId: `tab-<terminalId>`.
    expect(terminalDisplay(state(), { terminalId: 'term-a' })).toBe('working')
    // A surfaced heartbeat tab attaches under its own agent name.
    expect(terminalDisplay(state(), { terminalId: 'term-z', attachAgentName: 'cortana-hb' })).toBe('monitoring')
    expect(terminalDisplay(state(), { terminalId: 'nope' })).toBe('idle')
  })

  it('a pinned Chat with no tab still lights its workspace', async () => {
    snapshots.push({
      ...snap(1, [row('sid-chat', 'working', { agentName: 'p7', projectId: 'p7', workspacePath: '/srv/seven' })]),
      workspaces: [rollup('working', 'p7', '/srv/seven')],
    })
    attachActivity(SCOPE)
    await settle()
    expect(rowForAgentName(state(), 'p7')?.display).toBe('working')
    expect(workspaceDisplay(state(), { projectId: 'p7' })).toBe('working')
    expect(workspaceDisplay(state(), { projectId: 'other' })).toBe('idle')
  })
})

describe('notifications (T-S5d, RL11, A36)', () => {
  const toaster: ActivityToaster = { needsYou: vi.fn(), finished: vi.fn() }

  async function start(rows: ActivityRow[]): Promise<void> {
    vi.useFakeTimers()
    snapshots.push(snap(1, rows))
    attachActivity(SCOPE)
    registerActivityNotify(SCOPE, { toaster, projects: () => [], root: null })
    await vi.advanceTimersByTimeAsync(0)
    vi.mocked(toaster.needsYou).mockClear()
    vi.mocked(toaster.finished).mockClear()
  }

  const ended = (reason: string, outcome: 'success' | 'cancelled' | 'boundary' = 'success') => ({
    turnEnded: { outcome, reason, at: Date.now() },
  })

  it('a turn end then working again within 1.5 s: no toast, no chime, no mark', async () => {
    await start([row('s1', 'working')])
    await vi.advanceTimersByTimeAsync(SPAWN_GRACE_MS)
    emit(frame(2, row('s1', 'idle'), ended('turn_done')))
    await vi.advanceTimersByTimeAsync(NOTIFY_DEBOUNCE_MS - 100)
    emit(frame(3, row('s1', 'working')))
    await vi.advanceTimersByTimeAsync(CHIME_DEBOUNCE_MS * 2)
    expect(toaster.finished).not.toHaveBeenCalled()
    expect(playCompletionSound).not.toHaveBeenCalled()
    expect(state().unseenDone.size).toBe(0)
  })

  it('a settled turn end toasts at 1.5 s and marks + chimes at 4 s', async () => {
    await start([row('s1', 'working')])
    await vi.advanceTimersByTimeAsync(SPAWN_GRACE_MS)
    emit(frame(2, row('s1', 'idle'), ended('turn_done')))
    await vi.advanceTimersByTimeAsync(NOTIFY_DEBOUNCE_MS)
    expect(toaster.finished).toHaveBeenCalledTimes(1)
    expect(playCompletionSound).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(CHIME_DEBOUNCE_MS)
    expect(playCompletionSound).toHaveBeenCalledTimes(1)
    expect(vi.mocked(playCompletionSound).mock.calls[0][0]).toBe('p1')
    expect(projectHasUnseen(state(), 'p1')).toBe(true)
    expect(agentHasUnseen(state(), 'tab-s1')).toBe(true)
    // Looking at it clears the mark.
    setViewing(SCOPE, 'tab-s1', true)
    expect(projectHasUnseen(state(), 'p1')).toBe(false)
    setViewing(SCOPE, 'tab-s1', false)
  })

  it('boundary and cancelled ends never toast or chime', async () => {
    await start([row('s1', 'working'), row('s2', 'working')])
    await vi.advanceTimersByTimeAsync(SPAWN_GRACE_MS)
    emit(frame(2, row('s1', 'idle'), ended('session_boundary', 'boundary')))
    emit(frame(3, row('s2', 'idle'), ended('interrupted', 'cancelled')))
    await vi.advanceTimersByTimeAsync(CHIME_DEBOUNCE_MS * 2)
    expect(toaster.finished).not.toHaveBeenCalled()
    expect(playCompletionSound).not.toHaveBeenCalled()
  })

  it('a turn the lead ended while subagents run notifies when the row settles', async () => {
    await start([row('s1', 'working')])
    await vi.advanceTimersByTimeAsync(SPAWN_GRACE_MS)
    // Lead Stop, subagents still running: display stays working.
    emit(frame(2, row('s1', 'working', { reason: 'subagents_running' }), ended('turn_done')))
    await vi.advanceTimersByTimeAsync(CHIME_DEBOUNCE_MS * 2)
    expect(toaster.finished).not.toHaveBeenCalled()
    emit(frame(3, row('s1', 'idle', { reason: 'child_done' })))
    await vi.advanceTimersByTimeAsync(NOTIFY_DEBOUNCE_MS)
    expect(toaster.finished).toHaveBeenCalledTimes(1)
  })

  it('a session being looked at never toasts, marks or chimes', async () => {
    await start([row('s1', 'working')])
    await vi.advanceTimersByTimeAsync(SPAWN_GRACE_MS)
    setViewing(SCOPE, 'tab-s1', true)
    emit(frame(2, row('s1', 'idle'), ended('turn_done')))
    emit(frame(3, row('s1', 'waiting')))
    await vi.advanceTimersByTimeAsync(CHIME_DEBOUNCE_MS * 2)
    expect(toaster.finished).not.toHaveBeenCalled()
    expect(toaster.needsYou).not.toHaveBeenCalled()
    expect(playCompletionSound).not.toHaveBeenCalled()
    setViewing(SCOPE, 'tab-s1', false)
  })

  it('needs you toasts after 1.5 s, unless the wait resolves first', async () => {
    await start([row('s1', 'working'), row('s2', 'working')])
    emit(frame(2, row('s1', 'waiting')))
    emit(frame(3, row('s2', 'waiting')))
    await vi.advanceTimersByTimeAsync(500)
    emit(frame(4, row('s2', 'working')))
    await vi.advanceTimersByTimeAsync(NOTIFY_DEBOUNCE_MS)
    expect(toaster.needsYou).toHaveBeenCalledTimes(1)
    expect(vi.mocked(toaster.needsYou).mock.calls[0][0].sessionId).toBe('s1')
  })

  it('a turn that ends inside the spawn grace toasts but never marks or chimes', async () => {
    await start([])
    emit(frame(2, row('s1', 'working')))
    emit(frame(3, row('s1', 'idle'), ended('turn_done')))
    await vi.advanceTimersByTimeAsync(CHIME_DEBOUNCE_MS * 2)
    expect(toaster.finished).toHaveBeenCalledTimes(1)
    expect(playCompletionSound).not.toHaveBeenCalled()
    expect(state().unseenDone.size).toBe(0)
  })

  it('a pinned room host only chimes for rows under its root', async () => {
    vi.useFakeTimers()
    snapshots.push(snap(1, [row('in', 'working'), row('out', 'working', { workspacePath: '/srv/other' })]))
    attachActivity(SCOPE)
    // MS21: the chime reads THIS room's server's project record.
    const serverProjects = [{ id: 'p1', completionSoundEnabled: 1 }]
    registerActivityNotify(SCOPE, { toaster: null, projects: () => serverProjects, root: '/srv/ws' })
    await vi.advanceTimersByTimeAsync(SPAWN_GRACE_MS)
    emit(frame(2, row('out', 'idle', { workspacePath: '/srv/other' }), ended('turn_done')))
    await vi.advanceTimersByTimeAsync(CHIME_DEBOUNCE_MS)
    expect(playCompletionSound).not.toHaveBeenCalled()
    emit(frame(3, row('in', 'idle'), ended('turn_done')))
    await vi.advanceTimersByTimeAsync(CHIME_DEBOUNCE_MS)
    expect(playCompletionSound).toHaveBeenCalledTimes(1)
    expect(vi.mocked(playCompletionSound).mock.calls[0]).toEqual(['p1', serverProjects])
  })
})

describe('Active (T-S5f, RL12/A33)', () => {
  it('lead → working frames never write to the server: the daemon touches Active', async () => {
    snapshots.push(snap(1, [row('s1', 'idle')]))
    attachActivity(SCOPE)
    await settle()
    for (let seq = 2; seq < 8; seq++) emit(frame(seq, row('s1', seq % 2 === 0 ? 'working' : 'idle')))
    expect(mustRow('s1').display).toBe('idle')
    expect(daemonCliPost).not.toHaveBeenCalled()
  })
})
