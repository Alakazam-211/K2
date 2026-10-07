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
  HelloEvent,
  SessionActivityChangedEvent,
} from '@/stores/session-events'

type Handlers = {
  activity: Array<(e: ActivityChangedEvent) => void>
  hello: Array<(e: HelloEvent) => void>
  resync: Array<() => void>
  legacy: Array<(e: SessionActivityChangedEvent) => void>
}
const bus: Handlers = { activity: [], hello: [], resync: [], legacy: [] }

function add<T>(list: T[], fn: T): () => void {
  list.push(fn)
  return () => {
    const i = list.indexOf(fn)
    if (i >= 0) list.splice(i, 1)
  }
}

vi.mock('@/stores/session-events', () => ({
  onActivityChanged: (_s: unknown, fn: (e: ActivityChangedEvent) => void) => add(bus.activity, fn),
  onAppHello: (_s: unknown, fn: (e: HelloEvent) => void) => add(bus.hello, fn),
  onAppResync: (_s: unknown, fn: () => void) => add(bus.resync, fn),
  onSessionActivityChanged: (_s: unknown, fn: (e: SessionActivityChangedEvent) => void) => add(bus.legacy, fn),
}))

/** Snapshot replies, in order; a pull with none queued fails the test. */
const snapshots: ActivitySnapshot[] = []
const daemonCliGet = vi.fn(async (_scope: unknown, route: string): Promise<ActivitySnapshot> => {
  if (route !== 'activity/snapshot') throw new Error(`unexpected GET ${route}`)
  const next = snapshots.shift()
  if (!next) throw new Error('a snapshot was pulled but the test queued none')
  return next
})
const daemonCliPost = vi.fn(async () => ({}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (scope: unknown, route: string) => daemonCliGet(scope, route),
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
  displayUnderRoot,
  projectHasUnseen,
  registerActivityNotify,
  resetActivity,
  rowForAgentName,
  rowForTerminal,
  setViewing,
  terminalDisplay,
  workspaceDisplay,
  type ActivityToaster,
} from './activity'
import type { ServerScope } from '@/kessel/server-scope'

let supports = true
const SCOPE = { id: 'host:test', serverSupports: () => supports } as unknown as ServerScope
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
  snapshots.length = 0
  supports = true
})

afterEach(() => {
  __resetActivityForTests()
  vi.useRealTimers()
  if (snapshots.length !== 0) throw new Error(`${snapshots.length} queued snapshot(s) were never pulled`)
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

describe('an older server (Q5, RL13, T-S5e)', () => {
  it('shows its session_activity_changed stream as-is, with no snapshot pull', () => {
    supports = false
    attachActivity(SCOPE)
    hello()
    expect(daemonCliGet).not.toHaveBeenCalled()
    for (const fn of bus.legacy) {
      fn({ kind: 'session_activity_changed', workspacePath: '/srv/old', agentName: 'tab-t1', paneGroupId: 't1', status: 'permission' })
    }
    expect(state().supported).toBe(false)
    const r = rowForAgentName(state(), 'tab-t1')
    if (!r) throw new Error('no legacy row')
    expect(r.display).toBe('waiting')
    expect(terminalDisplay(state(), { terminalId: 't1' })).toBe('waiting')
    expect(workspaceDisplay(state(), { path: '/srv/old' })).toBe('waiting')
    expect(displayUnderRoot(state(), '/srv')).toBe('waiting')
  })

  it('a server with daemon activity ignores the compat stream', async () => {
    snapshots.push(snap(1, []))
    attachActivity(SCOPE)
    await settle()
    for (const fn of bus.legacy) {
      fn({ kind: 'session_activity_changed', workspacePath: '/srv/ws', agentName: 'tab-x', paneGroupId: null, status: 'working' })
    }
    expect(state().rows.size).toBe(0)
  })

  it('the first activity_changed from a server thought old switches to daemon rows', async () => {
    supports = false
    attachActivity(SCOPE)
    for (const fn of bus.legacy) {
      fn({ kind: 'session_activity_changed', workspacePath: '/srv/ws', agentName: 'tab-x', paneGroupId: null, status: 'working' })
    }
    snapshots.push(snap(4, [row('s1', 'idle')]))
    emit(frame(4, row('s1', 'idle')))
    await settle()
    expect(state().supported).toBe(true)
    expect([...state().rows.keys()]).toEqual(['s1'])
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
