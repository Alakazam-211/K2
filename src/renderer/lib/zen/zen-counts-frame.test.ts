// @vitest-environment jsdom
// 0.45.2 (Rosson 2026-10-08: "say how many ghosts are working and how many
// scary things (tool calls) have happened"): the activity counts reach a
// sealed widget frame end to end, for a built-in (k2:diary@2) and a user
// custom widget alike. Every link is K2's real code except the daemon:
//   activity store rows (as the daemon's frames land them) → zen-data
//   `zenAgentRows` (ZenAgentRow.counts) → the custom layer
//   (`createZenCustomLayer`, the guest projection) → the frame host
//   (`startZenFrameHost`, a real MessageChannel) → K2's frame runtime
//   (`k2FrameScript`, evaluated in its own realm) → the widget's script.
// Fixtures are made up (the repo is public).
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { createContext, runInContext } from 'node:vm'
import { MessageChannel } from 'node:worker_threads'
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
import { __resetActivityForTests, __seedActivityForTests, type ActivityDisplay, type ActivityRow } from '@/stores/activity'
import type { ZenWidgetBridge } from './zen-bridge'
import { createZenCustomLayer } from './zen-custom-bridge'
import { startZenFrameHost, type ZenFrameHost } from './zen-custom-host'
import { parseZenCustomWidget } from './zen-custom-payload'
import { k2FrameScript } from './zen-custom-prelude'
import type { ZenCustomWidgetPayload, ZenFrameHello } from './zen-custom-types'
import { __resetZenDataForTests, zenAgentRows, zenCustomViewFor, type ZenAgentRow, type ZenThreadView } from './zen-data'

const ZEN = resolve(__dirname, '../../../../crates/k2-core/src/zen')
const read = (p: string): string => readFileSync(resolve(ZEN, p), 'utf8')

const HOME: Home = { id: 'home-test', name: 'Test', rows: [{ address: 'cortana::local', workspaceId: 'p1', label: 'Cortana' }] }
const GARDEN = 'g-test0001'

/** The window server's activity for cortana's workspace: the rollup says
 *  `display`, and one session row carries `counts` (absent: an older K2). */
function activity(display: ActivityDisplay, counts?: ActivityRow['counts']): void {
  const row: ActivityRow = {
    ...(counts ? { counts } : {}),
    sessionId: 'sess-1',
    agentName: 'tab-sess-1',
    projectId: 'p1',
    workspacePath: '/w/cortana',
    harness: 'claude',
    display,
    lead: { state: display === 'idle' ? 'idle' : 'working', outcome: 'none', since: 0, promptId: null },
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
      {
        projectId: 'p1',
        workspacePath: '/w/cortana',
        display,
        counts: { working: 0, monitoring: 0, waiting: 0, unverifiable: 0, idle: 0, [display]: 1 },
        since: null,
      },
    ],
  })
}

function widget(source: 'builtin' | 'user'): ZenCustomWidgetPayload {
  const caps = ['agents:read', 'thread:read', 'thread:post']
  return parseZenCustomWidget(
    {
      kind: 'custom',
      widget: source === 'builtin' ? 'k2:diary@2' : 'ghost-counter',
      requested: caps,
      caps,
      name: source === 'builtin' ? 'Diary' : 'Ghost counter',
      hash: 'h',
      state: 'ok',
      origin: 'local',
      source,
    },
    { id: 'w1', column: 0, props: {} },
  )
}

/** The rows zen-data gives this widget now (what `agents.subscribe` sends). */
function rowsFor(w: ZenCustomWidgetPayload): ZenAgentRow[] {
  return zenAgentRows(zenCustomViewFor(GARDEN, w))
}

const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 5))
async function until(what: string, ok: () => boolean): Promise<void> {
  for (let i = 0; i < 200; i++) {
    if (ok()) return
    await tick()
  }
  throw new Error(`timed out waiting for: ${what}`)
}

type Listener = (e: unknown) => void

interface Wired {
  /** Push zen-data's rows now to the widget's live `agents.subscribe`. */
  pushRows(rows: ZenAgentRow[]): void
  /** Push a Thread view to the widget's `thread.subscribe`. */
  pushThread(view: ZenThreadView): void
  host: ZenFrameHost
}

const hosts: ZenFrameHost[] = []

/**
 * One sealed frame, wired the way ZenCustomFrame wires it: K2's runtime,
 * the widget's script, then the host's hello with the port. K2's own verbs
 * (the inner bridge) hand their callbacks to the test, which feeds them
 * zen-data's real rows.
 */
async function wire(w: ZenCustomWidgetPayload, html: string, run: (k2: unknown) => void): Promise<Wired> {
  document.body.innerHTML = html.slice(html.indexOf('<body>') + 6, html.indexOf('<script'))
  const listeners = new Map<string, Listener[]>()
  const parent = {}
  const g: Record<string, unknown> = {
    addEventListener: (t: string, f: Listener) => listeners.set(t, [...(listeners.get(t) ?? []), f]),
    removeEventListener: (t: string, f: Listener) => listeners.set(t, (listeners.get(t) ?? []).filter((x) => x !== f)),
    setTimeout: () => 0,
    parent,
    navigator: { platform: 'MacIntel' },
    document,
    console,
  }
  g.self = g
  runInContext(k2FrameScript(), createContext(g))
  if (!g.k2) throw new Error('the runtime set no k2')
  run(g.k2)

  let rowsCb: ((rows: ZenAgentRow[]) => void) | null = null
  let threadCb: ((v: ZenThreadView) => void) | null = null
  const inner = {
    widgetId: w.id,
    caps: new Set(w.caps),
    call: (verb: string, ...args: unknown[]) => {
      if (verb === 'agents.subscribe') {
        rowsCb = args[0] as (rows: ZenAgentRow[]) => void
        return () => undefined
      }
      if (verb === 'agents.list') return rowsFor(w)
      if (verb === 'thread.subscribe') {
        threadCb = args[1] as (v: ZenThreadView) => void
        return () => undefined
      }
      return null
    },
  } as unknown as ZenWidgetBridge

  await tick() // the frame "loads"
  const hello: ZenFrameHello = {
    k2: 'hello',
    v: 1,
    caps: [...w.caps],
    features: ['zen-v1', 'zen-gardens-v1', 'zen-widgets-v1'],
    widget: { id: w.id, name: w.name ?? 'W', garden: GARDEN },
    config: {},
    motion: { reduced: true },
  }
  const host = startZenFrameHost(
    {
      postMessage: (message, _origin, transfer) => {
        for (const f of listeners.get('message') ?? []) f({ source: parent, data: message, ports: transfer })
      },
    },
    {
      hello,
      label: w.widget,
      stop: (reason) => {
        throw new Error(`the widget stopped: ${reason}`)
      },
      focused: () => false,
      onChord: () => undefined,
      channel: () => new MessageChannel() as unknown as globalThis.MessageChannel,
      makeLayer: (push) =>
        createZenCustomLayer({
          widget: () => w,
          gardenId: () => GARDEN,
          inner,
          boundRows: () => rowsFor(w),
          ensureConversation: async () => undefined,
          push,
          stop: (reason) => {
            throw new Error(`the layer stopped the widget: ${reason}`)
          },
          paused: () => false,
          pauseForRunaway: () => {
            throw new Error('runaway')
          },
          theme: () => ({ theme: { scheme: 'dark', vars: {} }, chrome: { corners: null, stoplights: null }, motion: { reduced: true } }),
          onThemeChange: () => () => undefined,
          now: () => Date.now(),
        }),
    },
  )
  hosts.push(host)
  await until('the widget subscribes to agents', () => rowsCb !== null)
  return {
    host,
    pushRows: (rows) => {
      if (!rowsCb) throw new Error('no agents.subscribe')
      rowsCb(rows)
    },
    pushThread: (view) => {
      if (!threadCb) throw new Error('no thread.subscribe')
      threadCb(view)
    },
  }
}

let saved: { homes: Home[]; selected: string }

beforeEach(() => {
  saved = { homes: useHomesStore.getState().homes, selected: useHomesStore.getState().selectedId }
  useHomesStore.setState({ homes: [HOME], selectedId: HOME.id })
  useConnectHostStore.setState({ activeHost: 'local', hosts: [], connectionStatus: 'connected' })
  useProjectsStore.setState({
    projects: [{ id: 'p1', name: 'Cortana', handle: 'cortana', path: '/w/cortana', pinned: false } as never],
  })
  __resetActivityForTests()
  __resetZenDataForTests()
})

afterEach(() => {
  for (const h of hosts.splice(0)) h.dispose()
  useHomesStore.setState({ homes: saved.homes, selectedId: saved.selected })
  __resetActivityForTests()
  __resetZenDataForTests()
  document.body.innerHTML = ''
  document.documentElement.className = ''
})

describe('0.45.2 counts: the activity store to the row a widget sees', () => {
  it('a working agent’s row carries its session’s counts; idle, or an older K2, carries none', () => {
    const w = widget('user')
    activity('working', { subagents: 2, tools: 14, commands: 5 })
    const [row] = rowsFor(w)
    expect(row?.address).toBe('cortana::local')
    expect(row?.activity).toBe('working')
    expect(row?.counts).toEqual({ subagents: 2, tools: 14, commands: 5 })
    // An idle session's finished turn is not "now".
    activity('idle', { subagents: 0, tools: 14, commands: 5 })
    expect(rowsFor(w)[0]?.counts).toEqual({ subagents: 0, tools: 0, commands: 0 })
    // A 0.45.1 server: rows without counts add nothing.
    activity('working')
    expect(rowsFor(w)[0]?.counts).toEqual({ subagents: 0, tools: 0, commands: 0 })
  })
})

describe.each(['user', 'builtin'] as const)('0.45.2 counts reach a %s widget’s frame', (source) => {
  it('agents.subscribe in the frame gets exactly the three numbers, and their changes', async () => {
    const w = widget(source)
    const got: unknown[][] = []
    const f = await wire(w, '<html><body><div id="x"></div><script></script></body></html>', (k2) => {
      ;(k2 as { agents: { subscribe(cb: (rows: unknown[]) => void): void } }).agents.subscribe((rows) => got.push(rows))
    })
    activity('working', { subagents: 3, tools: 14, commands: 5 })
    f.pushRows(rowsFor(w))
    await until('rows in the frame', () => got.length === 1)
    const first = got[0][0] as Record<string, unknown>
    expect(first.address).toBe('cortana::local')
    expect(first.counts).toEqual({ subagents: 3, tools: 14, commands: 5 })
    activity('working', { subagents: 1, tools: 15, commands: 5 })
    f.pushRows(rowsFor(w))
    await until('the change in the frame', () => got.length === 2)
    expect((got[1][0] as Record<string, unknown>).counts).toEqual({ subagents: 1, tools: 15, commands: 5 })
  })
})

describe('0.45.2 counts: k2:diary@2 on the real path says how many ghosts and scary things', () => {
  const runDiary = (k2: unknown): void => {
    new Function('window', 'document', 'requestAnimationFrame', 'cancelAnimationFrame', 'performance', read('builtin_widgets/diary-2/diary.js'))(
      { k2 },
      document,
      () => 0,
      () => {},
      { now: () => 0 },
    )
  }
  const thread = (turn: ZenThreadView['turn']): ZenThreadView => ({
    address: 'cortana::local',
    phase: 'ready',
    note: null,
    items: [
      {
        seq: 1,
        id: 'm1',
        doc: { id: 'm1', kind: 'text', from: 'alice', body: 'are you there?', via: 'compose', created_at: 1_800_000_001 },
      } as unknown as ZenThreadView['items'][number],
    ],
    loaded: true,
    hasMore: false,
    loadingOlder: false,
    error: null,
    turn,
  })

  it('3 ghosts and 14 scary things while working; nothing once the turn ends; nothing from an older K2', async () => {
    const w = widget('builtin')
    const f = await wire(w, read('builtin_widgets/diary-2/index.html'), runDiary)
    const line = (): HTMLElement => {
      const el = document.getElementById('ghosts')
      if (!el) throw new Error('no #ghosts')
      return el
    }
    activity('working', { subagents: 3, tools: 14, commands: 5 })
    f.pushRows(rowsFor(w))
    await until('a page for Cortana', () => document.getElementById('who')?.textContent === 'Cortana')
    f.pushThread(thread({ state: 'working', since: 1 }))
    await until('the ghost line', () => !line().hidden)
    expect(line().textContent).toMatch(/\bthree ghosts\b/)
    expect(line().textContent).toMatch(/\b14 scary things\b/)

    activity('working', { subagents: 0, tools: 1, commands: 0 })
    f.pushRows(rowsFor(w))
    await until('one scary thing', () => /\b1 scary thing\b/.test(line().textContent ?? ''))
    expect(line().textContent).not.toMatch(/ghost|scary things/)

    // An older K2 (no counts): the scribble stays, the line says nothing.
    activity('working')
    f.pushRows(rowsFor(w))
    await until('the line goes', () => line().hidden)
    expect(line().textContent).toBe('')
    expect(document.getElementById('stir')?.hidden).toBe(false)
  })
})
