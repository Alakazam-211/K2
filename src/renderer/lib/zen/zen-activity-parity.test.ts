// 0.46.5 activity SDK, Garden parity (Rosson 2026-10-27: "Improve the Garden
// to get at least the same thing that we have available for the apps").
//
// A custom widget with thread:read + activity:read (`k2.thread.onActivity`)
// gets the SAME turn an App guest with the same caps gets from
// `thread.activity` and its `thread_activity` frames. The shared fixture is
// also read by the daemon test `guest_turn_parity_fixture`
// (thread_activity.rs), which runs `guest_turn_body` over the same owner
// bodies: both sides must produce each case's `guest`, word lists only.
import { describe, expect, it } from 'vitest'
import fixture from '../../../../tests/fixtures/activity/guest-turn-parity.json'
import { coerceThreadTurn } from '@/components/SessionView/overlayThread'
import type { ZenWidgetBridge } from './zen-bridge'
import type { ZenCustomWidgetPayload } from './zen-custom-types'
import type { ZenAgentRow, ZenThreadView } from './zen-data'
import { createZenCustomLayer, type ZenCustomLayerDeps } from './zen-custom-bridge'
import {
  projectZenActivityState,
  projectZenActivityTurn,
  projectZenThreadView,
  ZEN_ACTIVITY_TURN_KEYS,
} from './zen-custom-projection'
import { ZEN_CUSTOM_VERBS } from './zen-verbs.generated'

interface Case {
  name: string
  owner: Record<string, unknown>
  guest: Record<string, unknown>
}

const CASES = fixture.cases as Case[]
const SECRETS = fixture.secrets as string[]
const STATES = ['working', 'monitoring', 'needs-you', 'unverifiable', 'stopped', 'idle']
const PHASES = ['delivering', 'working', 'tool', 'thinking', 'waiting', 'stale', 'children']

/** The owner body as the Garden's feed holds it (the overlay socket frame,
 *  coerced on arrival). */
function feedTurn(owner: Record<string, unknown>) {
  const t = coerceThreadTurn(owner, Number(owner.serverNow) - 1_500)
  if (!t) throw new Error(`the fixture's owner body doesn't coerce: ${JSON.stringify(owner)}`)
  return t
}

function view(address: string, owner: Record<string, unknown> | null | undefined, over: Partial<ZenThreadView> = {}): ZenThreadView {
  return {
    address,
    phase: 'ready',
    note: null,
    items: [],
    loaded: true,
    hasMore: false,
    loadingOlder: false,
    error: null,
    ...(owner === undefined ? {} : { liveTurn: owner === null ? null : feedTurn(owner) }),
    ...over,
  }
}

/** Only fixed words, numbers, null and the turn id: no label, line, path. */
function assertWordListsOnly(turn: Record<string, unknown>): void {
  expect(Object.keys(turn).sort()).toEqual([...ZEN_ACTIVITY_TURN_KEYS].sort())
  expect(STATES).toContain(turn.state)
  expect(PHASES).toContain(turn.phase)
  expect([null, 'owner']).toContain(turn.waitingOn)
  const end = turn.end as { reason: string } | null
  if (end !== null) {
    expect(Object.keys(end)).toEqual(['reason'])
    expect(['done', 'stopped', 'failed']).toContain(end.reason)
  }
  const text = JSON.stringify(turn)
  for (const s of [...SECRETS, '`', '"line"', '"detail"', '"children":', '"label"', '"since"', 'lastComposeAt', 'replied']) {
    expect(text, `${s} must not reach a widget`).not.toContain(s)
  }
}

describe('the widget turn is the App guest turn (shared fixture)', () => {
  it('keeps its cases', () => {
    expect(CASES.length).toBeGreaterThanOrEqual(8)
  })

  for (const c of CASES) {
    it(c.name, () => {
      const turn = projectZenActivityTurn(feedTurn(c.owner))
      expect(turn).toEqual(c.guest)
      assertWordListsOnly(turn as unknown as Record<string, unknown>)
    })
  }
})

describe('k2.thread.onActivity through the custom layer', () => {
  function payload(caps: string[]): ZenCustomWidgetPayload {
    return {
      id: 'bar',
      kind: 'custom',
      widget: 'activity-bar',
      column: 0,
      props: { config: {} },
      caps: caps as ZenCustomWidgetPayload['caps'],
      requested: caps as ZenCustomWidgetPayload['requested'],
      source: 'user',
      name: 'Activity bar',
      description: null,
      reasons: {},
      libs: [],
      hash: 'h1',
      state: 'ok',
      errors: [],
      warnings: [],
      origin: 'local',
      paused: null,
    }
  }

  function harness(caps: string[]) {
    const pushes: Array<{ sub: number; value: unknown }> = []
    const errors: Array<{ sub: number; error: unknown }> = []
    const feeds = new Map<string, (v: ZenThreadView) => void>()
    const ensured: string[] = []
    const inner = {
      widgetId: 'bar',
      caps: new Set<string>(),
      call: (verb: string, ...args: unknown[]) => {
        if (verb === 'thread.subscribe') {
          feeds.set(String(args[0]), args[1] as (v: ZenThreadView) => void)
          return () => feeds.delete(String(args[0]))
        }
        throw new Error(`unexpected inner call ${verb}`)
      },
    } as unknown as ZenWidgetBridge
    const deps: ZenCustomLayerDeps = {
      widget: () => payload(caps),
      gardenId: () => 'g-test0001',
      inner,
      // The widget's agents (needAddress refuses any other: not_bound).
      boundRows: () => [{ address: 'alice::local' }, { address: 'bob::box.example.test' }] as unknown as ZenAgentRow[],
      ensureConversation: async (address: string) => {
        ensured.push(address)
      },
      push: (sub, value) => pushes.push({ sub, value }),
      pushError: (sub, error) => errors.push({ sub, error }),
      stop: () => undefined,
      paused: () => false,
      pauseForRunaway: () => undefined,
      theme: () => ({ theme: { scheme: 'light', vars: {} }, chrome: { corners: null, stoplights: null }, motion: { reduced: false } }),
      onThemeChange: () => () => undefined,
      now: () => 1_000_000,
    }
    return { layer: createZenCustomLayer(deps), pushes, errors, feeds, ensured }
  }

  const BOTH = ['agents:read', 'thread:read', 'activity:read']

  it('the catalog row needs both caps, like the App route', () => {
    expect(ZEN_CUSTOM_VERBS['thread.onActivity']).toEqual({ cap: 'activity:read', also: ['thread:read'], reach: 'portable', kind: 'subscribe' })
  })

  it('pushes the App guest turn for every fixture frame, word lists only', async () => {
    const h = harness(BOTH)
    expect(await h.layer.handle({ sub: 3, verb: 'thread.onActivity', args: ['alice::local'] })).toBeNull()
    expect(h.ensured).toEqual(['alice::local'])
    const feed = h.feeds.get('alice::local')!
    expect(feed).toBeTypeOf('function')
    for (const c of CASES) feed(view('alice::local', c.owner))
    const values = h.pushes.map((p) => p.value as Record<string, unknown>)
    // The first push is the snapshot (live case 0); every later frame is
    // pushed as is, end frames included.
    expect(values).toHaveLength(CASES.length)
    values.forEach((v, i) => {
      const c = CASES[i]!
      expect(v.turn, c.name).toEqual(c.guest)
      expect(v).toMatchObject({ address: 'alice::local', source: 'thread', fresh: 'live', live: (c.guest.end ?? null) === null })
      expect(Object.keys(v).sort()).toEqual(['address', 'fresh', 'live', 'source', 'turn'])
      assertWordListsOnly(v.turn as Record<string, unknown>)
    })
  })

  it('a snapshot of an ended turn reads as no turn (as thread.activity answers null); the same value is pushed once', async () => {
    const h = harness(BOTH)
    await h.layer.handle({ sub: 4, verb: 'thread.onActivity', args: ['alice::local'] })
    const feed = h.feeds.get('alice::local')!
    const ended = CASES.find((c) => c.guest.end !== null)!
    feed(view('alice::local', ended.owner))
    // A later view of the same feed (a new message item, a refocus) still
    // holds that old ended turn: it stays "no turn", never a late end frame.
    feed(view('alice::local', ended.owner, { hasMore: true }))
    expect(h.pushes.map((p) => p.value)).toEqual([{ address: 'alice::local', turn: null, live: false, source: 'thread', fresh: 'live' }])
    // The next turn shows, and so does its own end frame.
    const live = CASES[0]!
    feed(view('alice::local', live.owner))
    feed(view('alice::local', { ...ended.owner, turnId: live.owner.turnId, rev: 99 }))
    expect(h.pushes.map((p) => (p.value as { turn: unknown }).turn)).toEqual([null, live.guest, { ...ended.guest, turnId: live.guest.turnId, rev: 99 }])
  })

  it('a server without Thread turns says source none, never a guessed turn', async () => {
    const h = harness(BOTH)
    await h.layer.handle({ sub: 5, verb: 'thread.onActivity', args: ['bob::box.example.test'] })
    const feed = h.feeds.get('bob::box.example.test')!
    feed(view('bob::box.example.test', undefined, { loaded: false, phase: 'opening' }))
    feed(view('bob::box.example.test', undefined))
    expect(h.pushes.map((p) => p.value)).toEqual([
      { address: 'bob::box.example.test', turn: null, live: false, source: 'none', fresh: 'catching-up' },
      { address: 'bob::box.example.test', turn: null, live: false, source: 'none', fresh: 'live' },
    ])
  })

  it('refuses without activity:read, and without thread:read, naming the missing cap', async () => {
    for (const [caps, missing] of [
      [['agents:read', 'thread:read'], 'activity:read'],
      [['agents:read', 'activity:read'], 'thread:read'],
    ] as const) {
      const h = harness([...caps])
      // A refused subscription is answered once as {sub, error}.
      const r = (await h.layer.handle({ sub: 9, verb: 'thread.onActivity', args: ['alice::local'] })) as unknown as { sub: number; error: unknown }
      expect(r, `caps ${caps.join(',')}`).toMatchObject({ sub: 9, error: { code: 'cap_not_granted', cap: missing } })
      expect(h.feeds.size).toBe(0)
      expect(h.pushes).toEqual([])
    }
  })
})

describe('review B4: a Thread view keeps turn absent when the server reports none', () => {
  it('absent (undefined), null and a live turn', () => {
    const base = view('alice::local', undefined)
    expect('turn' in projectZenThreadView(base)).toBe(false)
    expect(projectZenThreadView({ ...base, turn: null }).turn).toBeNull()
    expect(projectZenThreadView({ ...base, turn: { state: 'working', since: 5 } }).turn).toEqual({ state: 'working', since: 5 })
  })

  it('projectZenActivityState is fresh retrying while the feed has an error', () => {
    expect(projectZenActivityState(view('alice::local', null, { error: 'socket closed' }), null)).toEqual({
      address: 'alice::local',
      turn: null,
      live: false,
      source: 'thread',
      fresh: 'retrying',
    })
  })
})
