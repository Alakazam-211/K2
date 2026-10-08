// prd-zen-user-widgets-v2 TUWB3 (as changed 2026-10-08: no scope to pick; a
// widget reaches the Garden's reach) and UW38 parsing. Fixtures are made up
// (the repo is public): hosts under example.test, Garden ids like g-test0001.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  gets: [] as Array<{ hostKey: string; route: string }>,
  rosters: {} as Record<string, unknown>,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string) => {
    h.gets.push({ hostKey: scope.hostKey, route })
    if (route !== 'projects/list') throw new Error(`unexpected GET ${route}`)
    const r = h.rosters[scope.hostKey]
    if (r === undefined) throw new Error(`no roster for ${scope.hostKey}`)
    return r
  }),
  daemonCliPost: vi.fn(async () => {
    throw new Error('unexpected POST')
  }),
}))

import { useHomesStore, type Home } from '@/stores/homes'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import {
  __resetZenScopeForTests,
  useZenServerRosters,
  zenServerName,
  zenWidgetReachRows,
  ZEN_WIDGET_MAX_SERVERS,
} from './zen-custom-scope'
import { parseZenCustomWidget, parseZenWidgetPause } from './zen-custom-payload'
import { parseZenGet } from './zen-page'
import { zenAgentRows, zenCustomViewFor } from './zen-data'

const WORK: Home = {
  id: 'home-work',
  name: 'Work',
  rows: [
    { address: 'alice::local', workspaceId: 'w1', label: 'Alice' },
    { address: 'bob::box.example.test', workspaceId: 'w2', label: 'Bob' },
  ],
}
const PLAY: Home = {
  id: 'home-play',
  name: 'Play',
  rows: [
    { address: 'carol::local', workspaceId: 'w3', label: 'Carol' },
    { address: 'alice::local', workspaceId: 'w1', label: 'Alice' },
  ],
}

function host(n: number): ConnectHost {
  return { id: `h${n}`, label: `Box ${n}`, hostname: `box${n}.example.test`, port: 443, secure: true, token: 'tok-test' } as ConnectHost
}

let saved: { homes: Home[]; hosts: ConnectHost[]; projects: unknown[]; active: unknown }

beforeEach(() => {
  saved = {
    homes: useHomesStore.getState().homes,
    hosts: useConnectHostStore.getState().hosts,
    projects: useProjectsStore.getState().projects,
    active: useConnectHostStore.getState().activeHost,
  }
  useHomesStore.setState({ homes: [WORK, PLAY], selectedId: WORK.id })
  useConnectHostStore.setState({ hosts: [], activeHost: 'local' })
  useProjectsStore.setState({ projects: [] })
  h.gets = []
  h.rosters = {}
  __resetZenScopeForTests()
})

afterEach(() => {
  useHomesStore.setState({ homes: saved.homes })
  useConnectHostStore.setState({ hosts: saved.hosts, activeHost: saved.active as 'local' })
  useProjectsStore.setState({ projects: saved.projects as never })
})

const addrs = (): string[] => zenWidgetReachRows().rows.map((r) => r.address)

describe('TUWB3: a widget reaches this computer plus every Home row, at once', () => {
  it("this computer's workspaces plus every Home's rows, deduplicated, local first", () => {
    useProjectsStore.setState({
      projects: [{ id: 'p9', name: 'Erin', handle: 'erin', path: '/x', pinned: false } as never],
    })
    expect(addrs()).toEqual(['alice::local', 'carol::local', 'erin::local', 'bob::box.example.test'])
    expect(h.gets).toEqual([])
  })

  it('an agent added to a Home appears at once; a deleted Home gives fewer rows', () => {
    expect(addrs()).toEqual(['alice::local', 'carol::local', 'bob::box.example.test'])
    useHomesStore.setState({
      homes: [WORK, { ...PLAY, rows: [...PLAY.rows, { address: 'dave::local', workspaceId: 'w4', label: 'Dave' }] }],
    })
    expect(addrs()).toContain('dave::local')
    useHomesStore.setState({ homes: [], selectedId: undefined })
    expect(addrs()).toEqual([])
  })

  it('a saved server that is on no Home is outside the reach (never listed, never fetched)', () => {
    useConnectHostStore.setState({ hosts: [host(1)], activeHost: 'local' })
    h.rosters['box1.example.test'] = [{ id: 'q1', name: 'Frank', handle: 'frank' }]
    expect(addrs()).not.toContain('frank::box1.example.test')
    expect(h.gets).toEqual([])
    // On a Home, a row of that server is in reach.
    useHomesStore.setState({ homes: [{ id: 'home-x', name: 'X', rows: [{ address: 'frank::box1.example.test', workspaceId: 'q1', label: 'Frank' }] }] })
    expect(addrs()).toEqual(['frank::box1.example.test'])
  })

  it("on another server's window, this computer's agents come from one local projects/list", async () => {
    useConnectHostStore.setState({ hosts: [host(1)], activeHost: host(1) as never })
    useHomesStore.setState({ homes: [], selectedId: undefined })
    h.rosters['local'] = [{ id: 'l1', name: 'Gina', handle: 'gina' }]
    expect(addrs()).toEqual([])
    await vi.waitFor(() => expect(useZenServerRosters.getState().rosters['local']?.status).toBe('ready'))
    expect(addrs()).toEqual(['gina::local'])
    // Cached: reading the reach again asks nothing more.
    const asked = h.gets.length
    addrs()
    await Promise.resolve()
    expect(h.gets.length).toBe(asked)
    expect(h.gets).toContainEqual({ hostKey: 'local', route: 'projects/list' })
  })

  it(`at most ${ZEN_WIDGET_MAX_SERVERS} live servers, this computer first; the rest are left out`, () => {
    const rows = Array.from({ length: 10 }, (_, i) => ({ address: `zed::box${i + 1}.example.test`, workspaceId: `z${i}`, label: 'Zed' }))
    useHomesStore.setState({ homes: [WORK, { id: 'home-many', name: 'Many', rows }] })
    const r = zenWidgetReachRows()
    expect(r.servers).toHaveLength(ZEN_WIDGET_MAX_SERVERS)
    expect(r.servers[0]).toBe('local')
    expect(r.overflow.length).toBeGreaterThan(0)
    for (const row of r.rows) expect(r.servers).toContain(row.address.split('::')[1])
  })

  it("a custom widget's rows are its reach; a widget that may not run (v4) gets none", () => {
    const base = { id: 'arcade', column: 0, props: {} }
    const raw = { kind: 'custom', widget: 'agent-arcade', requested: ['agents:read'], caps: ['agents:read'], hash: 'h', state: 'ok' }
    const local = parseZenCustomWidget({ ...raw, origin: 'local' }, base)
    expect(zenAgentRows(zenCustomViewFor('g-test0001', local)).map((r) => r.address)).toEqual([
      'alice::local',
      'carol::local',
      'bob::box.example.test',
    ])
    const other = parseZenCustomWidget({ ...raw, origin: 'imported' }, base)
    expect(zenAgentRows(zenCustomViewFor('g-test0001', other))).toEqual([])
  })

  it('server names', () => {
    expect(zenServerName('local')).toBe('this computer')
    useConnectHostStore.setState({ hosts: [host(2)], activeHost: 'local' })
    expect(zenServerName('box2.example.test')).toBe('Box 2')
  })
})

describe('UW38: parsing a custom widget', () => {
  const raw = {
    id: 'arcade',
    kind: 'custom',
    widget: 'agent-arcade',
    column: 0,
    props: { home: 'Work', config: { speed: 2, bad: { x: 1 } } },
    caps: ['agents:read', 'thread:read', 'gardens:manage'],
    requested: ['agents:read', 'thread:read', 'thread:post', 'gardens:manage'],
    source: 'builtin',
    name: 'Agent Arcade',
    description: 'Your agents as characters.',
    reasons: { 'thread:post': 'so you can talk', 'zen.exit': 'nope' },
    libs: ['three@0.170', { url: 'https://cdn.example.test/x.js', integrity: 'sha384-x' }, 7],
    hash: 'abc123',
    state: 'ok',
    errors: [],
    warnings: [{ file: 'widgets/agent-arcade/index.html', line: 3, col: 1, message: 'innerHTML' }],
    origin: 'local',
    paused: null,
  }

  it('caps = requested ∩ daemon caps ∩ widget caps, no grant; source is user', () => {
    const page = parseZenGet({
      ok: true,
      page: { template: 'k2.blank@1', layout: { split: [100] }, widgets: [raw], controls: [] },
    })
    const w = page.widgets[0]
    expect(w.kind).toBe('custom')
    expect(w.source).toBe('user')
    expect(w.caps).toEqual(['agents:read', 'thread:read'])
    expect(w.custom?.requested).toEqual(['agents:read', 'thread:read', 'thread:post'])
    expect(w.custom?.props).toEqual({ home: 'Work', config: { speed: 2 } })
    expect(w.custom?.reasons).toEqual({ 'thread:post': 'so you can talk' })
    expect(w.custom?.libs).toEqual(['three@0.170', { url: 'https://cdn.example.test/x.js', integrity: 'sha384-x' }])
    expect(w.custom?.origin).toBe('local')
    expect(w.custom?.paused).toBeNull()
    expect(w.custom).not.toHaveProperty('grant')
  })

  it('an origin that is not local (or unreadable) carries no caps: the v4 seam fails closed', () => {
    for (const origin of ['imported', 'bogus', undefined, 7]) {
      const c = parseZenCustomWidget({ ...raw, origin }, { id: 'a', column: 0, props: {} })
      expect(c.caps, String(origin)).toEqual([])
      expect(c.origin, String(origin)).toBe('other')
    }
    expect(parseZenCustomWidget({ ...raw, state: 'weird' }, { id: 'a', column: 0, props: {} }).state).toBe('broken')
  })

  it('the runaway pause: {at, reason: runaway} or null', () => {
    expect(parseZenWidgetPause({ at: '2026-10-08T00:00:00Z', reason: 'runaway' })).toEqual({ at: '2026-10-08T00:00:00Z', reason: 'runaway' })
    expect(parseZenWidgetPause({ reason: 'bored' })).toBeNull()
    expect(parseZenWidgetPause(null)).toBeNull()
    const c = parseZenCustomWidget({ ...raw, paused: { at: 't', reason: 'runaway' } }, { id: 'a', column: 0, props: {} })
    expect(c.paused).toEqual({ at: 't', reason: 'runaway' })
    expect(c.caps).toEqual(['agents:read', 'thread:read'])
  })
})
