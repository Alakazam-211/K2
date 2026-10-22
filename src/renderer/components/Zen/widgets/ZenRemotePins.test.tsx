// @vitest-environment jsdom
//
// Garden 1's pin grid keeps agents pinned on other servers (0.46.3 r9, a
// customer report: "Anna and Oaks will sometimes fall off the pinned
// chat"). The real Agents widget over zen-data's real rows (a fake bridge
// for the verbs only): a remote agent pinned on its own server stays in the
// grid with no room open, across a remount (re-entering Zen), and while its
// server's list is being re-read. Fixtures are made up (the repo is public).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import { cleanup, render } from '@testing-library/react'

const h = vi.hoisted(() => ({
  lists: {} as Record<string, unknown>,
  /** Set: `projects/list` answers only once it resolves (a read in flight). */
  gate: null as Promise<void> | null,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string) => {
    if (route === 'projects/get-icon') return { found: false, dataUrl: null }
    if (route !== 'projects/list') throw new Error(`unexpected GET ${route}`)
    if (h.gate) await h.gate
    const list = h.lists[scope.hostKey]
    if (list === undefined) throw new Error(`${scope.hostKey} is unreachable`)
    return list
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
  withHostCliSlot: vi.fn(async (_s: unknown, fn: () => Promise<unknown>) => fn()),
}))

import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import { useHomesStore, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { hostPool } from '@/lib/host-pool-instance'
import type { HostEntry } from '@/lib/host-pool'
import { __resetZenScopeForTests, requestZenServerRoster, useZenServerRosters } from '@/lib/zen/zen-custom-scope'
import { __resetZenDataForTests, zenAgentRows, type ZenAgentRow, type ZenView } from '@/lib/zen/zen-data'
import { ZEN_RECENT_FIRST_KEY, useZenRecentFirstStore } from '@/lib/zen/zen-recent-first'
import { ZenAgentsWidget } from './ZenAgentsWidget'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const BOX1 = 'box1.example.test'
const BOX2 = 'box2.example.test'
const PAT: HomeRow = { address: 'pat::local', workspaceId: 'wp', label: 'Pat' }
const ANNA: HomeRow = { address: `anna::${BOX1}`, workspaceId: 'wa', label: 'Anna' }
const OAKS: HomeRow = { address: `oaks::${BOX2}`, workspaceId: 'wo', label: 'Oaks' }
const SAM: HomeRow = { address: 'sam::local', workspaceId: 'ws', label: 'Sam' }
const HOME: Home = { id: 'home-1', name: 'Home', rows: [SAM, ANNA, PAT, OAKS] }
const VIEW: ZenView = { key: 'g-test0001/agents', gardenId: 'g-test0001', widgetId: 'agents', homeId: HOME.id, agent: null }

function saved(n: number): ConnectHost {
  return {
    id: `h${n}`,
    label: `Box ${n}`,
    hostname: `box${n}.example.test`,
    port: 443,
    secure: true,
    username: 'rosson',
    token: 'tok-test',
    remember: true,
    lastConnectedAt: null,
  } as ConnectHost
}

function live(hostKey: string): HostEntry {
  return {
    hostKey,
    saved: true,
    hostId: hostKey,
    reach: 'live',
    boot: { phase: 'ready', ready: true, version: '0.46.2', protocol: 1, instanceId: 'i1', features: [], at: 1 },
    auth: 'ok',
    authNote: null,
    role: 'member',
    presence: null,
    offlineStreak: 0,
    checkedAt: null,
  } as HostEntry
}

function list(...ws: Array<[id: string, handle: string, name: string, pinned: number]>): unknown[] {
  return ws.map(([id, handle, name, pinned]) => ({ id, handle, name, path: `/srv/${handle}`, pinned, workspaces: [] }))
}

/** The verbs the Agents widget calls; its rows are zen-data's real rows. */
function bridge(): ZenWidgetBridge {
  return {
    widgetId: 'agents',
    caps: new Set<string>(['agents:read']),
    call(verb: string): unknown {
      switch (verb) {
        case 'agents.list':
          return zenAgentRows(VIEW)
        case 'agents.subscribe':
          return () => {}
        case 'agents.home':
          return HOME.id
        default:
          throw new Error(`unexpected bridge verb ${verb}`)
      }
    },
    gardens: { list: () => [], current: () => ({ id: 'g-test0001', name: 'Garden 1' }) },
    homes: { list: () => [{ id: HOME.id, name: HOME.name, rows: HOME.rows.length }] },
    zen: { exit: () => {} },
  } as unknown as ZenWidgetBridge
}

/** Garden 1's Agents widget as the 0.46.x daemon sends it (`k2.texting@1`). */
const GARDEN1: ZenWidgetDecl = {
  id: 'agents',
  kind: 'agents',
  column: 0,
  props: { 'home-picker': false, pins: true, 'pin-limit': 9 },
  caps: ['agents:read'],
  source: 'builtin',
} as ZenWidgetDecl

const pinCells = (): string[] => Array.from(document.querySelectorAll('[data-zen-pin]')).map((e) => e.getAttribute('data-zen-pin') ?? '')
const listRows = (): string[] =>
  Array.from(document.querySelectorAll('[data-zen-agent-row]')).map((e) => e.getAttribute('data-zen-agent-row') ?? '')

async function rosterReady(hostKey: string): Promise<void> {
  await vi.waitFor(() => expect(useZenServerRosters.getState().rosters[hostKey]?.status).toBe('ready'))
}

function mount(): void {
  render(<ZenAgentsWidget bridge={bridge()} decl={GARDEN1} />)
}

let prev: { homes: Home[]; hosts: ConnectHost[]; projects: unknown[]; active: unknown }

beforeEach(() => {
  prev = {
    homes: useHomesStore.getState().homes,
    hosts: useConnectHostStore.getState().hosts,
    projects: useProjectsStore.getState().projects,
    active: useConnectHostStore.getState().activeHost,
  }
  localStorage.removeItem(ZEN_RECENT_FIRST_KEY)
  useZenRecentFirstStore.getState().reload()
  useHomesStore.setState({ homes: [HOME], selectedId: HOME.id })
  useConnectHostStore.setState({ hosts: [saved(1), saved(2)], activeHost: 'local', connectionStatus: 'connected' } as never)
  useProjectsStore.setState({ projects: list(['wp', 'pat', 'Pat', 1], ['ws', 'sam', 'Sam', 0]) as never })
  hostPool.store.setState({ entries: { [BOX1]: live(BOX1), [BOX2]: live(BOX2) } })
  h.lists = { [BOX1]: list(['wa', 'anna', 'Anna', 1]), [BOX2]: list(['wo', 'oaks', 'Oaks', 1]) }
  h.gate = null
  __resetZenScopeForTests()
  __resetZenDataForTests()
})

afterEach(() => {
  cleanup()
  __resetZenDataForTests()
  __resetZenScopeForTests()
  hostPool.store.setState({ entries: {} })
  useHomesStore.setState({ homes: prev.homes })
  useConnectHostStore.setState({ hosts: prev.hosts, activeHost: prev.active as 'local' })
  useProjectsStore.setState({ projects: prev.projects as never })
})

describe('Garden 1 pin grid: agents pinned on other servers', () => {
  it('keeps Anna and Oaks in the grid with no room open, across a remount and a list refresh', async () => {
    // The first read of the rows asks each remote server's list once.
    expect(zenAgentRows(VIEW).map((r: ZenAgentRow) => r.state)).toEqual(['ok', 'ok', 'ok', 'ok'])
    await rosterReady(BOX1)
    await rosterReady(BOX2)

    mount()
    expect(pinCells()).toEqual([ANNA.address, PAT.address, OAKS.address])
    expect(listRows()).toEqual([SAM.address])

    // Re-entering Zen: the widget unmounts and mounts again.
    cleanup()
    expect(pinCells()).toEqual([])
    mount()
    expect(pinCells()).toEqual([ANNA.address, PAT.address, OAKS.address])

    // The preview poll re-reads both lists: no blink while in flight.
    cleanup()
    let release: () => void = () => {
      throw new Error('gate not set')
    }
    h.gate = new Promise<void>((resolve) => {
      release = resolve
    })
    requestZenServerRoster(BOX1, Date.now(), { refresh: true })
    requestZenServerRoster(BOX2, Date.now(), { refresh: true })
    await Promise.resolve()
    expect(useZenServerRosters.getState().rosters[BOX1]?.status).toBe('loading')
    expect(useZenServerRosters.getState().rosters[BOX2]?.status).toBe('loading')
    mount()
    expect(pinCells()).toEqual([ANNA.address, PAT.address, OAKS.address])
    h.gate = null
    await act(async () => release())
    await rosterReady(BOX1)
    await rosterReady(BOX2)
    cleanup()
    mount()
    expect(pinCells()).toEqual([ANNA.address, PAT.address, OAKS.address])
  })

  it('an unpin on Oaks’ server takes it out of the grid on the next refresh', async () => {
    zenAgentRows(VIEW)
    await rosterReady(BOX1)
    await rosterReady(BOX2)
    h.lists[BOX2] = list(['wo', 'oaks', 'Oaks', 0])
    requestZenServerRoster(BOX2, Date.now(), { refresh: true })
    await vi.waitFor(() => expect(useZenServerRosters.getState().rosters[BOX2]?.workspaces[0]?.pinned).toBe(false))
    mount()
    expect(pinCells()).toEqual([ANNA.address, PAT.address])
    expect(listRows()).toEqual([SAM.address, OAKS.address])
  })
})
