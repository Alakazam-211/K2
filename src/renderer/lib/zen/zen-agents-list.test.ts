// 0.46.3 r10 (prd-garden-column-collapse-and-agent-list-v1 A2, CA11–CA13,
// §8): an Agents widget's chosen list (`list-from`). A Board (a custom
// widget) calls `k2.agents.show([...addresses])`; the Agents widget beside
// it draws the stored list ∩ the Garden's reach NOW, in list order.
//   - the custom layer: full row addresses only (a bare handle is
//     `not_bound`, naming it), at most 50, `bad_args` for anything else;
//     `show(null)` clears; the list clears when the widget stops or unloads;
//   - zen-data: "Nothing listed yet" before the first show (never the whole
//     reach); a row whose server drops out comes back with it; remote Home
//     rows are listed with their own state; an identical list is a no-op;
//     off-list agents that need you come flagged for the footer, never
//     selected; ⌘1–9 follows the list; the Agents rail view ignores it.
// Fixtures are made up (example.test, g-test0001).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))

import { useHomesStore, type Home } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { primaryScope } from '@/kessel/server-scope'
import { __resetActivityForTests, __seedActivityForTests, type ActivityRow } from '@/stores/activity'
import type { ZenVerbCtx, ZenWidgetBridge } from './zen-bridge'
import { createZenCustomLayer, type ZenCustomLayerDeps } from './zen-custom-bridge'
import type { ZenFrameReply } from './zen-custom-types'
import { ZEN_AGENTS_SHOW_MAX } from './zen-custom-types'
import { parseZenGet, type ZenResolvedPage } from './zen-page'
import {
  __resetZenDataForTests,
  zenAgentRows,
  zenAgentsShow,
  zenViewFor,
  type ZenAgentRow,
} from './zen-data'
import { parseZenCustomWidget } from './zen-custom-payload'

const G = 'g-test0001'

const WORK: Home = {
  id: 'home-work',
  name: 'Work',
  rows: [
    { address: 'alice::local', workspaceId: 'p1', label: 'Alice' },
    { address: 'bob::local', workspaceId: 'p2', label: 'Bob' },
    { address: 'dave::box.example.test', workspaceId: 'r1', label: 'Dave' },
  ],
}
const PLAY: Home = {
  id: 'home-play',
  name: 'Play',
  rows: [
    { address: 'carol::local', workspaceId: 'p3', label: 'Carol' },
    { address: 'frank::box2.example.test', workspaceId: 'r2', label: 'Frank' },
  ],
}

const BOARD = {
  id: 'board',
  kind: 'custom',
  widget: 'board-builder',
  column: 0,
  props: { config: {} },
  caps: ['agents:read', 'thread:read', 'thread:post'],
  requested: ['agents:read', 'thread:read', 'thread:post'],
  name: 'Board',
  hash: 'h',
  state: 'ok',
  origin: 'local',
  paused: null,
}

function page(crewProps: Record<string, unknown> = { mode: 'list', 'list-from': 'board' }, board: Record<string, unknown> = BOARD): ZenResolvedPage {
  return parseZenGet({
    ok: true,
    page: {
      template: 'k2.blank@1',
      layout: { split: [60, 40] },
      controls: [],
      widgets: [
        board,
        { id: 'crew', kind: 'agents', column: 1, props: crewProps, caps: ['agents:read', 'presence:read'], source: 'builtin' },
        { id: 'talk', kind: 'conversation', column: 1, props: { agents: 'crew' }, caps: ['thread:read'], source: 'builtin' },
      ],
    },
  })
}

function ctx(p: ZenResolvedPage, widgetId: string): ZenVerbCtx {
  return { widgetId, page: () => p, gardenId: () => G }
}

const addrs = (rows: ZenAgentRow[]): string[] => rows.map((r) => r.address)

/** Workspace `projectId` on the window's server needs you (a permission
 *  prompt), as the daemon's activity frame lands it. */
function needsYou(projectId: string, path: string): void {
  const row: ActivityRow = {
    sessionId: `sess-${projectId}`,
    agentName: `tab-${projectId}`,
    projectId,
    workspacePath: path,
    harness: 'claude',
    display: 'waiting',
    lead: { state: 'working', outcome: 'none', since: 0, promptId: null },
    children: { subagents: 0, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 },
    turnStartedAt: null,
    evidenceAt: 1,
    evidenceSource: 'hook',
    reason: 'turn_running',
    staleSince: null,
    confirmed: true,
    rev: 1,
  }
  __seedActivityForTests(primaryScope(), {
    supported: true,
    rows: [row],
    workspaces: [
      { projectId, workspacePath: path, display: 'waiting', counts: { working: 0, monitoring: 0, waiting: 1, unverifiable: 0, idle: 0 }, since: null },
    ],
  })
}

let saved: { homes: Home[]; selected: string }

beforeEach(() => {
  saved = { homes: useHomesStore.getState().homes, selected: useHomesStore.getState().selectedId }
  useHomesStore.setState({ homes: [WORK, PLAY], selectedId: WORK.id })
  useConnectHostStore.setState({ activeHost: 'local', hosts: [], connectionStatus: 'connected' })
  useProjectsStore.setState({
    projects: [
      { id: 'p1', name: 'Alice', handle: 'alice', path: '/w/alice', pinned: false } as never,
      { id: 'p2', name: 'Bob', handle: 'bob', path: '/w/bob', pinned: false } as never,
      { id: 'p3', name: 'Carol', handle: 'carol', path: '/w/carol', pinned: false } as never,
    ],
  })
  __resetActivityForTests()
  __resetZenDataForTests()
})

afterEach(() => {
  useHomesStore.setState({ homes: saved.homes, selectedId: saved.selected })
  __resetActivityForTests()
  __resetZenDataForTests()
})

describe('zen-data: the list view', () => {
  it('is keyed by its custom widget: the Board, the list and a following Conversation share one pick', () => {
    const p = page()
    const crew = zenViewFor(p, G, 'crew')
    expect(crew?.source).toBe('list')
    expect(crew?.key).toBe(zenViewFor(p, G, 'board')?.key)
    expect(zenViewFor(p, G, 'talk')?.key).toBe(crew?.key)
    expect(crew?.homeId).toBeNull()
  })

  it('before the first show, and after show(null): nothing listed, never the whole reach', () => {
    const p = page()
    const crew = zenViewFor(p, G, 'crew')
    expect(zenAgentRows(crew)).toEqual([])
    expect(zenAgentsShow(ctx(p, 'board'), ['carol::local'])).toBeTypeOf('function')
    expect(addrs(zenAgentRows(crew))).toEqual(['carol::local'])
    expect(zenAgentsShow(ctx(p, 'board'), null)).toBeNull()
    expect(zenAgentRows(crew)).toEqual([])
  })

  it('draws the list in its order (index = list position), remote Home rows included with their own state', () => {
    const p = page()
    zenAgentsShow(ctx(p, 'board'), ['dave::box.example.test', 'carol::local', 'alice::local'])
    const rows = zenAgentRows(zenViewFor(p, G, 'crew'))
    expect(addrs(rows)).toEqual(['dave::box.example.test', 'carol::local', 'alice::local'])
    expect(rows.map((r) => r.index)).toEqual([0, 1, 2])
    expect(rows[0].server).toBe('box.example.test')
    expect(rows[0].state).not.toBe('ok')
    expect(rows.every((r) => r.offList === undefined)).toBe(true)
  })

  it('the stored list ∩ the reach now: a row whose Home drops out comes back with it', () => {
    const p = page()
    zenAgentsShow(ctx(p, 'board'), ['frank::box2.example.test', 'alice::local'])
    const crew = zenViewFor(p, G, 'crew')
    // Frank's server is on no Home now: out of the reach, so not drawn.
    useHomesStore.setState({ homes: [WORK] })
    expect(addrs(zenAgentRows(crew))).toEqual(['alice::local'])
    useHomesStore.setState({ homes: [WORK, PLAY] })
    expect(addrs(zenAgentRows(crew))).toEqual(['frank::box2.example.test', 'alice::local'])
  })

  it('an identical list keeps the list held (no change); a new list replaces it; a stale release clears nothing', () => {
    const p = page()
    const first = zenAgentsShow(ctx(p, 'board'), ['alice::local'])!
    const again = zenAgentsShow(ctx(p, 'board'), ['alice::local'])!
    // The same list: either release clears it (it was never replaced).
    first()
    expect(zenAgentRows(zenViewFor(p, G, 'crew'))).toEqual([])
    zenAgentsShow(ctx(p, 'board'), ['alice::local'])
    const next = zenAgentsShow(ctx(p, 'board'), ['bob::local'])!
    again()
    expect(addrs(zenAgentRows(zenViewFor(p, G, 'crew')))).toEqual(['bob::local'])
    next()
    expect(zenAgentRows(zenViewFor(p, G, 'crew'))).toEqual([])
  })

  it('CA12: off-list agents that need you come after the list, flagged offList and never selected', () => {
    const p = page()
    needsYou('p2', '/w/bob')
    zenAgentsShow(ctx(p, 'board'), ['carol::local'])
    const rows = zenAgentRows(zenViewFor(p, G, 'crew'))
    expect(addrs(rows)).toEqual(['carol::local', 'bob::local'])
    expect(rows[1]).toMatchObject({ offList: true, selected: false, needsYou: true })
    expect(rows[0].offList).toBeUndefined()
    // Alice is off the list and doesn't need you: not there at all.
    expect(addrs(rows)).not.toContain('alice::local')
  })

  it('only a custom widget may show a list; a built-in Agents widget may not', () => {
    const p = page()
    expect(() => zenAgentsShow(ctx(p, 'crew'), ['alice::local'])).toThrow(/for custom widgets/)
    expect(() => zenAgentsShow(ctx(p, 'board'), 'alice::local')).toThrow(/a list of at most 50/)
    expect(() => zenAgentsShow(ctx(p, 'board'), [7])).toThrow(/a list of at most 50/)
  })

  it('a list whose custom widget may not run (v4, imported) shows nothing', () => {
    const p = page(undefined, { ...BOARD, origin: 'imported' })
    expect(zenViewFor(p, G, 'crew')?.reach).toBe(false)
    expect(zenAgentRows(zenViewFor(p, G, 'crew'))).toEqual([])
  })

  it('CA13: the Agents rail view (source: workspaces) ignores list-from', () => {
    const p = page({ mode: 'list', 'list-from': 'board', source: 'workspaces' })
    expect(zenViewFor(p, G, 'crew')?.source).toBe('workspaces')
  })

  it('an Agents widget without list-from is unchanged (a whole Home)', () => {
    const p = page({ mode: 'home', home: 'Play' })
    const v = zenViewFor(p, G, 'crew')
    expect(v?.source).toBeUndefined()
    expect(v?.homeId).toBe(PLAY.id)
  })
})

// ── the custom layer (K2's own verbs faked behind the inner bridge) ─────

const ALICE = { address: 'alice::local' } as ZenAgentRow
const DAVE = { address: 'dave::box.example.test' } as ZenAgentRow

function layerWith(released: string[], over: Partial<ZenCustomLayerDeps> = {}) {
  const calls: Array<{ verb: string; args: unknown[] }> = []
  const inner = {
    widgetId: 'board',
    caps: new Set<string>(),
    call: (verb: string, ...args: unknown[]) => {
      calls.push({ verb, args })
      if (verb === 'agents.show') return args[0] === null ? null : () => released.push(JSON.stringify(args[0]))
      throw new Error(`unexpected ${verb}`)
    },
  } as unknown as ZenWidgetBridge
  const widget = parseZenCustomWidget({ ...BOARD }, { id: 'board', column: 0, props: {} })
  const deps: ZenCustomLayerDeps = {
    widget: () => widget,
    gardenId: () => G,
    inner,
    boundRows: () => [ALICE, DAVE],
    ensureConversation: async () => undefined,
    push: () => undefined,
    stop: () => undefined,
    paused: () => false,
    pauseForRunaway: () => undefined,
    theme: () => ({}),
    onThemeChange: () => () => undefined,
    now: () => 1_000_000,
    ...over,
  }
  return { layer: createZenCustomLayer(deps), calls }
}

async function call(layer: ReturnType<typeof createZenCustomLayer>, verb: string, ...args: unknown[]): Promise<ZenFrameReply> {
  const r = await layer.handle({ id: 1, verb, args })
  if (!r) throw new Error('no reply')
  return r
}

function refusal(r: ZenFrameReply): { code: string; message: string; room?: string } {
  if (!('ok' in r) || r.ok) throw new Error(`not a refusal: ${JSON.stringify(r)}`)
  return r.error as { code: string; message: string; room?: string }
}

describe('the custom layer: k2.agents.show', () => {
  it('takes full row addresses from agents.list(), remote ones too; answers null', async () => {
    const { layer, calls } = layerWith([])
    expect(await call(layer, 'agents.show', ['dave::box.example.test', 'alice::local'])).toEqual({ id: 1, ok: true, value: null })
    expect(calls).toEqual([{ verb: 'agents.show', args: [['dave::box.example.test', 'alice::local']] }])
  })

  it('a bare handle, or an address outside the reach, is not_bound naming it; nothing reaches K2', async () => {
    const { layer, calls } = layerWith([])
    const bare = refusal(await call(layer, 'agents.show', ['alice::local', 'dave']))
    expect(bare.code).toBe('not_bound')
    expect(bare.message).toContain('dave')
    const outside = refusal(await call(layer, 'agents.show', ['erin::other.example.test']))
    expect(outside.code).toBe('not_bound')
    expect(outside.message).toContain('erin::other.example.test')
    expect(outside.room).toBe('erin')
    // Exact: no case folding or trimming.
    expect(refusal(await call(layer, 'agents.show', ['Alice::local'])).code).toBe('not_bound')
    expect(calls).toEqual([])
  })

  it('bad_args: not a list, not text, more than 50, or an address twice', async () => {
    const { layer, calls } = layerWith([])
    for (const bad of ['alice::local', { a: 1 }, [7], ['alice::local', 'alice::local'], Array(ZEN_AGENTS_SHOW_MAX + 1).fill('alice::local')]) {
      expect(refusal(await call(layer, 'agents.show', bad)).code, JSON.stringify(bad).slice(0, 40)).toBe('bad_args')
    }
    expect(calls).toEqual([])
  })

  it('null clears; the list clears when the widget stops or unloads', async () => {
    const released: string[] = []
    const { layer, calls } = layerWith(released)
    expect(await call(layer, 'agents.show', null)).toEqual({ id: 1, ok: true, value: null })
    expect(calls).toEqual([{ verb: 'agents.show', args: [null] }])
    await call(layer, 'agents.show', ['alice::local'])
    layer.dispose()
    expect(released).toEqual(['["alice::local"]'])
    // Once: a second dispose releases nothing more.
    layer.dispose()
    expect(released).toEqual(['["alice::local"]'])
  })

  it('needs agents:read', async () => {
    const widget = parseZenCustomWidget({ ...BOARD, caps: ['thread:read'], requested: ['thread:read'] }, { id: 'board', column: 0, props: {} })
    const { layer, calls } = layerWith([], { widget: () => widget })
    expect(refusal(await call(layer, 'agents.show', ['alice::local'])).code).toBe('cap_not_granted')
    expect(calls).toEqual([])
  })
})
