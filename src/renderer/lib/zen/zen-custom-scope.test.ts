// prd-zen-user-widgets-v2 TUWB3 (scope), UW38 parsing and UWA8 entries.
// Fixtures are made up (the repo is public): hosts under example.test,
// Garden ids like g-test0001.
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
  zenAskScope,
  zenGrantEntries,
  zenScopeRows,
  zenScopeWhere,
  ZEN_WIDGET_MAX_SERVERS,
} from './zen-custom-scope'
import { parseZenCustomWidget, parseZenGrantView, parseZenScope } from './zen-custom-payload'
import { parseZenGet } from './zen-page'

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
  return { id: `h${n}`, label: `Box ${n}`, hostname: `box${n}.example.test`, port: 443, secure: true } as ConnectHost
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

const addrs = (s: ReturnType<typeof zenScopeRows>): string[] => s.rows.map((r) => r.address)

describe('TUWB3: each scope kind resolves to the right rows', () => {
  it('agent, home, homes and allHomes', () => {
    expect(addrs(zenScopeRows({ agent: 'bob::box.example.test' }))).toEqual(['bob::box.example.test'])
    expect(addrs(zenScopeRows({ home: 'home-work' }))).toEqual(['alice::local', 'bob::box.example.test'])
    expect(addrs(zenScopeRows({ homes: ['home-play', 'home-work'] }))).toEqual([
      'carol::local',
      'alice::local',
      'bob::box.example.test',
    ])
    expect(addrs(zenScopeRows({ allHomes: true }))).toEqual(['alice::local', 'bob::box.example.test', 'carol::local'])
    expect(zenScopeRows(null).rows).toEqual([])
  })

  it('an agent added to a granted Home appears at once; a deleted Home gives fewer rows, never the window Home', () => {
    expect(addrs(zenScopeRows({ home: 'home-play' }))).toEqual(['carol::local', 'alice::local'])
    useHomesStore.setState({
      homes: [WORK, { ...PLAY, rows: [...PLAY.rows, { address: 'dave::local', workspaceId: 'w4', label: 'Dave' }] }],
    })
    expect(addrs(zenScopeRows({ home: 'home-play' }))).toEqual(['carol::local', 'alice::local', 'dave::local'])
    useHomesStore.setState({ homes: [WORK], selectedId: WORK.id })
    expect(addrs(zenScopeRows({ home: 'home-play' }))).toEqual([])
    expect(addrs(zenScopeRows({ homes: ['home-play', 'home-work'] }))).toEqual(['alice::local', 'bob::box.example.test'])
  })

  it('server: its Home rows plus its workspaces (the window server from the projects store)', () => {
    useProjectsStore.setState({
      projects: [{ id: 'p9', name: 'Erin', handle: 'erin', path: '/x', pinned: false } as never],
    })
    expect(addrs(zenScopeRows({ server: 'local' }))).toEqual(['alice::local', 'carol::local', 'erin::local'])
    expect(h.gets).toEqual([])
  })

  it('server: another server reads projects/list once with your login there', async () => {
    useConnectHostStore.setState({ hosts: [host(1)], activeHost: 'local' })
    h.rosters['box1.example.test'] = [{ id: 'q1', name: 'Frank', handle: 'frank' }]
    expect(addrs(zenScopeRows({ server: 'box1.example.test' }))).toEqual([])
    await vi.waitFor(() => expect(useZenServerRosters.getState().rosters['box1.example.test']?.status).toBe('ready'))
    expect(addrs(zenScopeRows({ server: 'box1.example.test' }))).toEqual(['frank::box1.example.test'])
    zenScopeRows({ server: 'box1.example.test' })
    expect(h.gets).toEqual([{ hostKey: 'box1.example.test', route: 'projects/list' }])
  })

  it(`allServers: this computer first, at most ${ZEN_WIDGET_MAX_SERVERS} live servers, the rest left out`, async () => {
    const hosts = Array.from({ length: 9 }, (_, i) => host(i + 1))
    useConnectHostStore.setState({ hosts, activeHost: 'local' })
    for (const x of hosts) h.rosters[`${x.hostname}`] = [{ id: 'z', name: 'Zed', handle: `zed-${x.id}` }]
    zenScopeRows({ allServers: true })
    await vi.waitFor(() =>
      expect(Object.values(useZenServerRosters.getState().rosters).filter((r) => r.status === 'ready')).toHaveLength(9),
    )
    const r = zenScopeRows({ allServers: true })
    expect(r.servers).toHaveLength(ZEN_WIDGET_MAX_SERVERS)
    expect(r.servers[0]).toBe('local')
    expect(r.overflow.length).toBeGreaterThan(0)
    for (const row of r.rows) expect(r.servers).toContain(row.address.split('::')[1])
  })
})

describe('UWA8: grant entries', () => {
  it('one {server, room} per row, sorted, deduplicated', () => {
    expect(zenGrantEntries(zenScopeRows({ allHomes: true }).rows)).toEqual([
      { server: 'box.example.test', room: 'bob' },
      { server: 'local', room: 'alice' },
      { server: 'local', room: 'carol' },
    ])
  })
})

describe('UW22: the placement ask fixes the scope', () => {
  it('home by name or id; agent within that Home; a missing name says so', () => {
    expect(zenAskScope({ home: 'work' })).toEqual({ scope: { home: 'home-work' } })
    expect(zenAskScope({ home: 'Work', agent: 'Bob' })).toEqual({ scope: { agent: 'bob::box.example.test' } })
    expect(zenAskScope({ agent: 'carol' })).toEqual({ scope: { agent: 'carol::local' } })
    expect(zenAskScope({ home: 'Nope' })).toEqual({ missing: 'No Home called “Nope” on this computer.' })
    expect(zenAskScope({ home: 'Work', agent: 'carol' })).toEqual({ missing: 'No agent “carol” in Work.' })
    expect(zenAskScope({})).toBeNull()
  })

  it('where words', () => {
    expect(zenScopeWhere({ home: 'home-work' })).toBe('Work')
    expect(zenScopeWhere({ homes: ['home-work', 'home-play'] })).toBe('Work and Play')
    expect(zenScopeWhere({ allHomes: true })).toBe('all your Homes')
    expect(zenScopeWhere({ agent: 'carol::local' })).toBe('only Carol')
    expect(zenScopeWhere({ allServers: true })).toBe('every server you use')
    expect(zenScopeWhere({ server: 'local' })).toBe('this computer')
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
    grant: {
      state: 'partial',
      caps: ['agents:read', 'thread:read'],
      granted: ['agents:read', 'thread:read'],
      scope: { home: 'home-work' },
      entries: [{ server: 'local', room: 'alice' }],
      sending: true,
      paused: null,
      grantedAt: '2026-10-08T00:00:00Z',
      widgetHash: 'abc123',
    },
  }

  it('effective caps = requested ∩ daemon caps ∩ grant caps ∩ widget caps; source is user', () => {
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
    expect(w.custom?.grant?.state).toBe('partial')
  })

  it('a grant needing review (or unreadable) carries no caps', () => {
    for (const state of ['review', 'invalid', 'none', 'bogus']) {
      const c = parseZenCustomWidget({ ...raw, grant: { ...raw.grant, state } }, { id: 'a', column: 0, props: {} })
      expect(c.caps, state).toEqual([])
      expect(c.grant?.caps, state).toEqual([])
    }
    expect(parseZenCustomWidget({ ...raw, grant: null }, { id: 'a', column: 0, props: {} }).caps).toEqual([])
    expect(parseZenGrantView('x')).toBeNull()
    expect(parseZenCustomWidget({ ...raw, state: 'weird' }, { id: 'a', column: 0, props: {} }).state).toBe('broken')
  })

  it('scopes: exactly one known key', () => {
    expect(parseZenScope({ home: 'h' })).toEqual({ home: 'h' })
    expect(parseZenScope({ homes: ['a', 'b'] })).toEqual({ homes: ['a', 'b'] })
    expect(parseZenScope({ homes: ['a', 'a'] })).toBeNull()
    expect(parseZenScope({ homes: [] })).toBeNull()
    expect(parseZenScope({ allHomes: true })).toEqual({ allHomes: true })
    expect(parseZenScope({ allServers: false })).toBeNull()
    expect(parseZenScope({ home: 'a', server: 'b' })).toBeNull()
    expect(parseZenScope({ galaxy: 'x' })).toBeNull()
  })
})
