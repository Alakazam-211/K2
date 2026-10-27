// @k2/activity core (0.46.5 activity SDK): the model, the words and the
// live-state driver. Fixture turns come from the shared parity fixture the
// daemon test (guest_turn_parity_fixture) also reads.
import { describe as group, expect, it } from 'vitest'
import fixture from '../../../tests/fixtures/activity/guest-turn-parity.json'
import {
  coerceTurn,
  describe,
  drawerLine,
  reduceTurn,
  stripView,
  watchActivity,
  type ActivityState,
  type ActivityTransport,
  type Snapshot,
} from '../src/core'

type Case = { name: string; owner: Record<string, unknown>; guest: Record<string, unknown> }
const CASES = fixture.cases as Case[]
const byName = (part: string): Case => {
  const c = CASES.find((x) => x.name.includes(part))
  if (!c) throw new Error(`no fixture case like ${part}`)
  return c
}

group('coerceTurn', () => {
  it('reads every fixture body, owner and guest, on the server clock', () => {
    for (const c of CASES) {
      const owner = coerceTurn(c.owner, Number(c.owner.serverNow) - 2_000)
      const guest = coerceTurn(c.guest, Number(c.guest.serverNow) - 2_000)
      expect(owner?.viewer, c.name).toBe('owner')
      expect(guest?.viewer, c.name).toBe('guest')
      expect(owner?.skewMs).toBe(2_000)
      expect(guest?.skewMs).toBe(2_000)
      expect(guest?.children, 'a guest never has children by name').toBeNull()
      expect(guest?.step.text, 'a guest never has a line').toBeNull()
      expect(owner?.turnId).toBe(guest?.turnId)
      expect(owner?.state).toBe(guest?.state)
      expect(owner?.schema, c.name).toBe(c.owner.schema === 2 ? 2 : 1)
      expect(guest?.schema, 'every guest frame says schema 2').toBe(2)
    }
  })

  it('schema 2 (the daemon’s real frames): step kind, child counts, why it is open, evidence', () => {
    const g = coerceTurn(byName('tool step').guest, 1000072000)!
    expect(g.schema).toBe(2)
    expect(g.step).toEqual({ kind: 'cmd', text: null })
    expect(g.childCounts).toEqual({ subagent: 2, shell: 1, monitor: 0, task: 0 })
    expect(g.openBecause).toBe('lead')
    expect(g.evidenceAt, 'a guest’s evidence is floored to 10 s').toBe(1000070000)
    const o = coerceTurn(byName('tool step').owner, 1000072000)!
    expect(o.evidenceAt).toBe(1000071234)
    expect(o.step).toEqual({ kind: 'cmd', text: 'Running `cargo test --token sk-live-abc123`' })
    expect(coerceTurn(byName('permission').guest, 0)!.openBecause).toBe('waiting')
    expect(coerceTurn(byName('monitoring').guest, 0)!.openBecause).toBe('children')
    expect(coerceTurn(byName('unverifiable').guest, 0)!.openBecause).toBe('stale')
    expect(coerceTurn(byName('done').guest, 0)!.openBecause).toBeNull()
  })

  it('schema 1 (an older server): the owner line names the step kind; children count by kind; open-because from the phase', () => {
    const old = byName('older owner body')
    const owner = coerceTurn(old.owner, 0)!
    expect(owner.schema).toBe(1)
    expect(owner.step).toEqual({ kind: 'cmd', text: 'Running `cargo test --token sk-live-abc123`' })
    expect(owner.childCounts).toEqual({ subagent: 1, shell: 1, monitor: 0, task: 0 })
    expect(owner.openBecause).toBe('lead')
    expect(owner.evidenceAt).toBeNull()
    // A 0.46.3–0.46.4 guest body: counts only, background of no named kind.
    const legacy = { ...byName('monitoring').guest }
    for (const k of ['schema', 'stepKind', 'childCounts', 'openBecause', 'evidenceAt']) delete legacy[k]
    const g = coerceTurn(legacy, 0)!
    expect(g.schema).toBe(1)
    expect(g.childCounts).toEqual({ subagent: 0, shell: 0, monitor: 0, task: 1 })
    expect(g.openBecause).toBe('children')
  })

  it('W9: words from a newer server are kept as unknown, never dropped', () => {
    const t = coerceTurn({ ...byName('tool step').guest, state: 'pondering', phase: 'dreaming' }, 0)
    expect(t).not.toBeNull()
    expect([t!.state, t!.phase]).toEqual(['unknown', 'unknown'])
    const d = describe(t, 1000072000)!
    expect(d.headline).toBe('Working')
    expect(d.animate, 'an unknown word never animates').toBe(false)
  })

  it('a coerced turn (or the renderer’s own ThreadTurn) reads the same again', () => {
    const t = coerceTurn(byName('tool step').owner, 1000070000)!
    expect(coerceTurn(t, 5)).toEqual(t)
    const g = coerceTurn(byName('monitoring').guest, 1000100000)!
    expect(coerceTurn(g, 5)).toEqual(g)
  })

  it('not a turn: null', () => {
    expect(coerceTurn(null, 0)).toBeNull()
    expect(coerceTurn({ state: 'working' }, 0)).toBeNull()
    expect(coerceTurn([], 0)).toBeNull()
  })
})

group('reduceTurn (W2)', () => {
  it('the same turn only moves forward by rev; another turn replaces it', () => {
    const a = coerceTurn({ ...byName('tool step').guest, rev: 4 }, 0)!
    const older = coerceTurn({ ...byName('tool step').guest, rev: 3 }, 0)!
    const newer = coerceTurn({ ...byName('tool step').guest, rev: 5 }, 0)!
    const other = coerceTurn(byName('permission').guest, 0)!
    expect(reduceTurn(a, older)).toBe(a)
    expect(reduceTurn(a, newer)).toBe(newer)
    expect(reduceTurn(a, other)).toBe(other)
    expect(reduceTurn(null, a)).toBe(a)
    expect(reduceTurn(a, null)).toBe(a)
  })
})

group('words', () => {
  const at = (c: Case, who: 'owner' | 'guest', plusMs = 0) => {
    const raw = c[who]
    const t = coerceTurn(raw, Number(raw.serverNow))!
    return describe(t, Number(raw.serverNow) + plusMs)!
  }

  it('one line per turn, owner and guest (guests read word lists only)', () => {
    expect(at(byName('tool step'), 'owner').headline).toBe('Running `cargo test --token sk-live-abc123`')
    // A guest reads the step's kind (schema 2), never the line.
    expect(at(byName('tool step'), 'guest').headline).toBe('Running a command')
    // An older server's guest body has no kind: the bar says Working.
    expect(at(byName('older owner body'), 'guest').headline).toBe('Working')
    expect(at(byName('permission'), 'owner').headline).toBe('Stuck on a permission prompt')
    expect(at(byName('permission'), 'guest').headline).toBe('Waiting on the owner')
    expect(at(byName('question'), 'owner').headline).toBe('Stuck on a question')
    expect(at(byName('monitoring'), 'owner').headline).toBe("Replied · Running `npm run dev` in /Users/rosson/app in the background +1")
    expect(at(byName('monitoring'), 'guest').headline).toBe('Replied · 2 subagents done +1')
    expect(at(byName('done'), 'guest').headline).toBe('Done')
    expect(at(byName('superseded'), 'guest').headline).toBe('Stopped')
    expect(at(byName('never arrived'), 'guest').headline).toBe('Failed')
    expect(at(byName('done'), 'owner').headline).toBe('Done')
  })

  it('a guest’s tool step speaks its kind when the server names it (schema 2)', () => {
    const raw = { ...byName('tool step').guest, schema: 2, stepKind: 'search' }
    expect(describe(coerceTurn(raw, 1000072000), 1000072000)!.headline).toBe('Searching')
  })

  it('pulse only while the agent itself works: never for monitoring, needs-you, a quiet turn or an end (no typing dots)', () => {
    expect(at(byName('tool step'), 'guest').animate).toBe(true)
    expect(at(byName('monitoring'), 'guest').animate).toBe(false)
    expect(at(byName('monitoring'), 'guest').tone).toBe('monitoring')
    expect(at(byName('permission'), 'guest').animate).toBe(false)
    expect(at(byName('unverifiable'), 'guest').animate).toBe(false)
    expect(at(byName('done'), 'guest').animate).toBe(false)
  })

  it('No update in Nm counts from the evidence (schema 2) or from the stale phase', () => {
    const c = byName('unverifiable')
    // Schema 2: the evidence clock (1000000000, floored) read at 1002030500.
    const t = coerceTurn(c.guest, Number(c.guest.serverNow))!
    expect(describe(t, Number(c.guest.serverNow) + 30_000)!.headline).toBe('No update in 33m')
    // Schema 1 (no evidence): staleSince 1002000000 − 30 min, read 30 s later.
    const legacy = coerceTurn({ ...c.guest, evidenceAt: null }, Number(c.guest.serverNow))!
    expect(describe(legacy, Number(c.guest.serverNow) + 30_000)!.headline).toBe('No update in 30m')
  })

  it('the clock counts from the message on the server’s clock, whatever this clock says', () => {
    const c = byName('tool step')
    // This client is 1 h behind the server.
    const t = coerceTurn(c.guest, Number(c.guest.serverNow) - 3_600_000)!
    expect(describe(t, Number(c.guest.serverNow) - 3_600_000)!.clock).toBe('0:12')
  })

  it('counters and the detail come from the counts', () => {
    const d = at(byName('tool step'), 'guest')
    expect(d.counters.map((x) => x.key)).toEqual(['subagents', 'background', 'read', 'search', 'cmd'])
    expect(d.detail).toEqual([
      'Working · 12s since your message',
      'Running a command',
      'Read 3 files · searched 1 time · ran 2 commands',
      '2 subagents running, 1 done · 1 background task',
    ])
  })

  it('stripView / drawerLine: nothing for an idle end, Stopped for a fresh interrupt', () => {
    const c = byName('done')
    expect(stripView(coerceTurn(c.owner, Number(c.owner.serverNow))!, Number(c.owner.serverNow))).toBeNull()
    const stop = coerceTurn({ ...c.owner, end: { reason: 'interrupted', detail: null, at: c.owner.serverNow } }, Number(c.owner.serverNow))!
    expect(drawerLine(stop, Number(c.owner.serverNow) + 1_000)!.step.text).toBe('Stopped')
    expect(drawerLine(stop, Number(c.owner.serverNow) + 3_000)).toBeNull()
  })
})

// ── watchActivity (W1–W8) ───────────────────────────────────────────────

function fakeClock() {
  let t = 1_000_000
  const timers: Array<{ at: number; fn: () => void; id: number }> = []
  let next = 1
  return {
    now: () => t,
    setTimeout: (fn: () => void, ms: number) => {
      const id = next++
      timers.push({ at: t + ms, fn, id })
      return id
    },
    clearTimeout: (id: unknown) => {
      const i = timers.findIndex((x) => x.id === id)
      if (i >= 0) timers.splice(i, 1)
    },
    advance(ms: number) {
      const end = t + ms
      for (;;) {
        timers.sort((a, b) => a.at - b.at)
        const first = timers[0]
        if (!first || first.at > end) break
        timers.shift()
        t = first.at
        first.fn()
      }
      t = end
    },
    pending: () => timers.length,
  }
}

function fakeTransport() {
  let on: Parameters<ActivityTransport['connect']>[0] | null = null
  const reads: Array<{ resolve: (s: Snapshot) => void; reject: (e: unknown) => void }> = []
  let closed = 0
  const t: ActivityTransport = {
    connect(h) {
      on = h
      return () => {
        closed += 1
      }
    },
    read() {
      return new Promise<Snapshot>((resolve, reject) => reads.push({ resolve, reject }))
    },
  }
  return {
    t,
    get on() {
      if (!on) throw new Error('not connected')
      return on
    },
    reads,
    closed: () => closed,
  }
}

const flush = () => new Promise((r) => setTimeout(r, 0))
const turnOf = (s: Map<string, ActivityState> | undefined, k: string) => s?.get(k)?.turn ?? null

group('watchActivity', () => {
  const live = byName('tool step').guest
  const ended = byName('done').guest

  it('W1: socket first; a read on every open REPLACES the state', async () => {
    const clock = fakeClock()
    const tr = fakeTransport()
    const seen: Array<Map<string, ActivityState>> = []
    const off = watchActivity(tr.t, (m) => seen.push(m), { ...clock, onVisible: () => () => undefined })
    expect(tr.reads).toHaveLength(0)
    tr.on.open()
    expect(seen.at(-1)?.size).toBe(0)
    expect(tr.reads).toHaveLength(1)
    tr.reads[0]!.resolve({ sales: live, ops: null })
    await flush()
    expect(turnOf(seen.at(-1), 'sales')?.turnId).toBe('msg-1')
    expect(seen.at(-1)?.get('sales')).toMatchObject({ live: true, source: 'thread', fresh: 'live' })
    expect(turnOf(seen.at(-1), 'ops')).toBeNull()
    // A reconnect reads again and replaces: the turn is gone now.
    tr.on.drop()
    expect(seen.at(-1)?.get('sales')?.fresh).toBe('retrying')
    clock.advance(1_000)
    tr.on.open()
    tr.reads[1]!.resolve({ sales: null, ops: null })
    await flush()
    expect(turnOf(seen.at(-1), 'sales')).toBeNull()
    off()
  })

  it('W1: a frame that arrives while the read is in flight wins over the older read', async () => {
    const clock = fakeClock()
    const tr = fakeTransport()
    let last: Map<string, ActivityState> | undefined
    const off = watchActivity(tr.t, (m) => (last = m), { ...clock, onVisible: () => () => undefined })
    tr.on.open()
    tr.on.frame('sales', { ...live, rev: 9 })
    tr.reads[0]!.resolve({ sales: { ...live, rev: 4 } })
    await flush()
    expect(turnOf(last, 'sales')?.rev).toBe(9)
    off()
  })

  it('W2: an end frame drops the bar (after endLingerMs)', async () => {
    const clock = fakeClock()
    const tr = fakeTransport()
    let last: Map<string, ActivityState> | undefined
    const off = watchActivity(tr.t, (m) => (last = m), { ...clock, endLingerMs: 2_000, onVisible: () => () => undefined })
    tr.on.open()
    tr.reads[0]!.resolve({})
    await flush()
    tr.on.frame('sales', live)
    tr.on.frame('sales', { ...ended, turnId: live.turnId, rev: 99 })
    expect(turnOf(last, 'sales')?.end?.reason).toBe('done')
    expect(last?.get('sales')?.live).toBe(false)
    clock.advance(2_000)
    expect(turnOf(last, 'sales')).toBeNull()
    off()
  })

  it('W3: activity_reset re-reads at once; W5: at most one read a second', async () => {
    const clock = fakeClock()
    const tr = fakeTransport()
    const off = watchActivity(tr.t, () => undefined, { ...clock, onVisible: () => () => undefined })
    tr.on.open()
    tr.reads[0]!.resolve({})
    await flush()
    tr.on.reset()
    expect(tr.reads).toHaveLength(1)
    clock.advance(999)
    expect(tr.reads).toHaveLength(1)
    clock.advance(1)
    expect(tr.reads).toHaveLength(2)
    off()
  })

  it('W3: a reset that carries the turns now (the daemon’s activity_reset) replaces the state without a read: a lost end frame clears the bar', async () => {
    const clock = fakeClock()
    const tr = fakeTransport()
    let last: Map<string, ActivityState> | undefined
    const off = watchActivity(tr.t, (m) => (last = m), { ...clock, onVisible: () => () => undefined })
    tr.on.open()
    tr.reads[0]!.resolve({})
    await flush()
    tr.on.frame('sales', live)
    tr.on.frame('ops', live)
    expect(turnOf(last, 'sales')?.turnId).toBe('msg-1')
    // The room stream's reset: {kind: 'activity_reset', turns: {room: turn|null}}.
    tr.on.reset({ sales: null, ops: { ...live, rev: 12 } })
    expect(turnOf(last, 'sales')).toBeNull()
    expect(turnOf(last, 'ops')?.rev).toBe(12)
    expect(tr.reads).toHaveLength(1)
    off()
  })

  it('W4: a shown turn with no frame for 60 s is read again; nothing shown, no read', async () => {
    const clock = fakeClock()
    const tr = fakeTransport()
    const off = watchActivity(tr.t, () => undefined, { ...clock, onVisible: () => () => undefined })
    tr.on.open()
    tr.reads[0]!.resolve({ sales: null })
    await flush()
    clock.advance(60_000)
    expect(tr.reads).toHaveLength(1)
    tr.on.frame('sales', live)
    clock.advance(120_000)
    expect(tr.reads.length).toBeGreaterThanOrEqual(2)
    off()
  })

  it('W4: a 429 waits retryAfterMs before the next read', async () => {
    const clock = fakeClock()
    const tr = fakeTransport()
    const off = watchActivity(tr.t, () => undefined, { ...clock, onVisible: () => () => undefined })
    tr.on.open()
    tr.reads[0]!.reject({ code: 'rate_limited', retryAfterMs: 7_000 })
    await flush()
    clock.advance(6_999)
    expect(tr.reads).toHaveLength(1)
    clock.advance(1)
    expect(tr.reads).toHaveLength(2)
    off()
  })

  it('a visible page re-reads while a turn shows', async () => {
    const clock = fakeClock()
    const tr = fakeTransport()
    let visible: (() => void) | null = null
    const off = watchActivity(tr.t, () => undefined, {
      ...clock,
      onVisible: (fn) => {
        visible = fn
        return () => (visible = null)
      },
    })
    tr.on.open()
    tr.reads[0]!.resolve({ sales: live })
    await flush()
    clock.advance(5_000)
    visible!()
    expect(tr.reads).toHaveLength(2)
    off()
    expect(visible, 'unsubscribe drops the visibility hook').toBeNull()
  })

  it('W8: unsubscribe closes the socket, clears every timer and calls nothing after', async () => {
    const clock = fakeClock()
    const tr = fakeTransport()
    let calls = 0
    const off = watchActivity(tr.t, () => (calls += 1), { ...clock, onVisible: () => () => undefined })
    tr.on.open()
    const before = calls
    off()
    expect(tr.closed()).toBe(1)
    expect(clock.pending()).toBe(0)
    tr.reads[0]!.resolve({ sales: live })
    await flush()
    tr.on.frame('sales', live)
    tr.on.open()
    expect(calls).toBe(before)
  })
})
