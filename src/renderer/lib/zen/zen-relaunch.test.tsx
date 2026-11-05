// @vitest-environment jsdom
//
// Rosson 2026-11-05: "If K2 restarts while widgets are running in the Fleet
// Ops 2 Garden, relaunch the widgets after the restart is done." A daemon
// restart (a new instance id, then ready) re-reads the page and remounts
// every custom frame once; a same-instance reconnect does nothing; a restart
// still booting waits for ready; a remote room's restart relaunches only the
// widgets that reach that server. Only the daemon is faked; fixtures are
// made up (example.test hosts, g-test0001).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({ bundleGets: 0 }))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string, params?: { widget?: string }) => {
    if (route !== 'zen/widget/bundle') throw new Error(`unexpected GET ${route}`)
    h.bundleGets += 1
    return {
      ok: true,
      widget: params?.widget,
      hash: 'hash-1',
      nonce: 'q83vFzPq1N3V0aZ8k2LmTw',
      html: '<!doctype html><html><body><script nonce="q83vFzPq1N3V0aZ8k2LmTw">k2.ready()</script></body></html>',
      bytes: 120,
    }
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))

import { act } from 'react'
import { cleanup, render } from '@testing-library/react'
import { useConnectHostStore } from '@/stores/connect-host'
import { useZenGardensStore } from './zen-gardens'
import { useZenConfigStore, __resetZenApiForTests } from './zen-api'
import { __resetZenCustomRunForTests, stopZenWidget, useZenCustomRunStore } from './zen-custom-run'
import { resetZenPausedStartForTests, takeZenPausedAtBoot } from './zen-widgets-running'
import type { ZenCustomWidgetPayload } from './zen-custom-types'
import type { ZenResolvedPage, ZenWidgetDecl } from './zen-page'
import type { ZenWidgetBridge } from './zen-bridge'
import {
  ZEN_RESTART_READY_DELAYS_MS,
  createZenRestartWatch,
  relaunchZenAfterRestart,
  zenWidgetServers,
  type ZenBootAnswer,
} from './zen-relaunch'
import { ZenCustomWidget } from '@/components/Zen/widgets/ZenCustomWidget'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const GARDEN = 'g-test0001'
const REMOTE = 'box.example.test'

function custom(id: string, over: Partial<ZenCustomWidgetPayload> = {}): ZenCustomWidgetPayload {
  return {
    id,
    kind: 'custom',
    widget: `w-${id}`,
    column: 0,
    props: { config: {} },
    caps: [],
    requested: [],
    source: 'user',
    name: id,
    description: null,
    reasons: {},
    libs: [],
    hash: 'hash-1',
    state: 'ok',
    errors: [],
    warnings: [],
    origin: 'local',
    paused: null,
    ...over,
  }
}

function decl(w: ZenCustomWidgetPayload): ZenWidgetDecl {
  return { id: w.id, kind: 'custom', column: 0, props: {}, caps: [...w.caps], source: 'user', custom: w }
}

function page(widgets: ZenCustomWidgetPayload[], version = 'v1'): ZenResolvedPage {
  return { schema: 1, version, widgets: widgets.map(decl) } as unknown as ZenResolvedPage
}

function showPage(p: ZenResolvedPage): void {
  useZenConfigStore.setState({ status: 'ready', gardenId: GARDEN, page: p, failure: null })
}

/** A fake clock for the watch's timers. */
function fakeTimers(): {
  setTimer(fn: () => void, ms: number): unknown
  clearTimer(t: unknown): void
  delays: number[]
  fire(): void
  pending(): number
} {
  const queue = new Map<number, () => void>()
  const delays: number[] = []
  let next = 1
  return {
    setTimer(fn, ms) {
      delays.push(ms)
      const id = next++
      queue.set(id, fn)
      return id
    },
    clearTimer(t) {
      queue.delete(t as number)
    },
    delays,
    fire() {
      const due = [...queue.entries()]
      queue.clear()
      if (due.length === 0) throw new Error('no timer to fire')
      for (const [, fn] of due) fn()
    },
    pending: () => queue.size,
  }
}

/** Let the watch's boot() promise chain settle. */
async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve()
}

beforeEach(() => {
  h.bundleGets = 0
  useConnectHostStore.setState({ hosts: [], activeHost: 'local' })
  useZenGardensStore.setState({ gardens: [{ id: GARDEN, name: 'Fleet Ops 2', template: 'k2.blank@1', index: 1 } as never] })
  __resetZenApiForTests()
  __resetZenCustomRunForTests()
  resetZenPausedStartForTests()
  takeZenPausedAtBoot(null, { label: 'main', storage: null })
})

afterEach(() => {
  cleanup()
})

describe('the restart watch: one relaunch per new instance id, once ready', () => {
  function setup(answers: Array<ZenBootAnswer | null>) {
    const timers = fakeTimers()
    const boots: string[] = []
    const relaunched: string[] = []
    const watch = createZenRestartWatch({
      boot: async (hostKey) => {
        boots.push(hostKey)
        if (answers.length === 0) throw new Error(`no boot answer queued for ${hostKey}`)
        return answers.shift() as ZenBootAnswer | null
      },
      relaunch: (hostKey) => relaunched.push(hostKey),
      setTimer: timers.setTimer,
      clearTimer: timers.clearTimer,
    })
    return { watch, timers, boots, relaunched }
  }

  it('a same-instance reconnect (a socket blip) does nothing: no status check, no relaunch', async () => {
    const t = setup([])
    t.watch.note('local', 'inst-a')
    t.watch.note('local', 'inst-a')
    t.watch.note('local', 'inst-a')
    await settle()
    expect(t.boots).toEqual([])
    expect(t.relaunched).toEqual([])
    expect(t.watch.waiting()).toEqual([])
    expect(t.timers.pending()).toBe(0)
  })

  it('a new instance id that is already ready relaunches once; later hellos of it do nothing', async () => {
    const t = setup([{ ready: true, instanceId: 'inst-b' }])
    t.watch.note('local', 'inst-a')
    t.watch.note('local', 'inst-b')
    await settle()
    expect(t.boots).toEqual(['local'])
    expect(t.relaunched).toEqual(['local'])
    t.watch.note('local', 'inst-b')
    t.watch.note('local', 'inst-b')
    await settle()
    expect(t.relaunched).toEqual(['local'])
    expect(t.boots).toEqual(['local'])
  })

  it('a restart while the daemon is still booting waits for ready, then relaunches exactly once', async () => {
    const t = setup([
      { ready: false, instanceId: 'inst-b' },
      null, // no answer at all (still binding its port)
      { ready: false, instanceId: 'inst-b' },
      { ready: true, instanceId: 'inst-b' },
    ])
    t.watch.note('local', 'inst-a')
    t.watch.note('local', 'inst-b')
    await settle()
    expect(t.relaunched).toEqual([])
    expect(t.watch.waiting()).toEqual(['local'])
    // More hellos while it boots fold into the same restart.
    t.watch.note('local', 'inst-b')
    await settle()
    expect(t.boots).toEqual(['local'])
    t.timers.fire()
    await settle()
    expect(t.relaunched).toEqual([])
    t.timers.fire()
    await settle()
    expect(t.relaunched).toEqual([])
    t.timers.fire()
    await settle()
    expect(t.relaunched).toEqual(['local'])
    expect(t.boots).toEqual(['local', 'local', 'local', 'local'])
    expect(t.timers.delays).toEqual(ZEN_RESTART_READY_DELAYS_MS.slice(0, 3))
    expect(t.watch.waiting()).toEqual([])
    expect(t.timers.pending()).toBe(0)
  })

  it('a second restart before the first was ready is still one relaunch', async () => {
    const t = setup([{ ready: false, instanceId: 'inst-b' }, { ready: true, instanceId: 'inst-c' }])
    t.watch.note('local', 'inst-a')
    t.watch.note('local', 'inst-b')
    await settle()
    t.watch.note('local', 'inst-c')
    await settle()
    t.timers.fire()
    await settle()
    expect(t.relaunched).toEqual(['local'])
    // Its instance is now the baseline: a reconnect to it does nothing.
    t.watch.note('local', 'inst-c')
    await settle()
    expect(t.relaunched).toEqual(['local'])
  })

  it('servers are separate: a remote room restarting relaunches only that server', async () => {
    const t = setup([{ ready: true, instanceId: 'r-2' }])
    t.watch.note('local', 'inst-a')
    t.watch.note(REMOTE, 'r-1')
    t.watch.note('local', 'inst-a')
    t.watch.note(REMOTE, 'r-2')
    await settle()
    expect(t.relaunched).toEqual([REMOTE])
  })

  it('the host pool’s restart event (prev → next) counts when no hello was seen yet', async () => {
    const t = setup([{ ready: true, instanceId: 'r-2' }])
    t.watch.note(REMOTE, 'r-2', 'r-1')
    await settle()
    expect(t.relaunched).toEqual([REMOTE])
  })

  it('an older daemon with no instance id never triggers a relaunch', async () => {
    const t = setup([])
    t.watch.note('local', undefined)
    t.watch.note('local', null)
    t.watch.note('local', '')
    await settle()
    expect(t.boots).toEqual([])
  })

  it('dispose drops a pending wait', async () => {
    const t = setup([{ ready: false, instanceId: 'inst-b' }])
    t.watch.note('local', 'inst-a')
    t.watch.note('local', 'inst-b')
    await settle()
    expect(t.timers.pending()).toBe(1)
    t.watch.dispose()
    expect(t.timers.pending()).toBe(0)
    t.watch.note('local', 'inst-c')
    await settle()
    expect(t.relaunched).toEqual([])
  })
})

describe('what a widget reaches', () => {
  it('the Garden’s servers when it reads agents or Threads, plus its data sources’ servers', () => {
    const reach = () => ['local', REMOTE]
    expect([...zenWidgetServers(custom('clock'), reach)]).toEqual([])
    expect([...zenWidgetServers(custom('fleet', { caps: ['agents:read'] }), reach)]).toEqual(['local', REMOTE])
    expect([...zenWidgetServers(custom('chat', { caps: ['thread:post'] }), reach)]).toEqual(['local', REMOTE])
    expect([...zenWidgetServers(custom('feed', { sourceAddresses: ['fleet::Other.Example.Test', 'local-feed'] }), reach)]).toEqual([
      'other.example.test',
    ])
  })
})

describe('relaunch after a restart', () => {
  it('Zen’s own server: re-reads the page once, clears stops, remounts every frame once', async () => {
    const fleet = custom('fleet', { sourceAddresses: [`fleet::${REMOTE}`] })
    const clock = custom('clock')
    showPage(page([fleet, clock]))
    stopZenWidget(`${GARDEN}/clock`, 'failing')
    let reloads = 0
    const keys = await relaunchZenAfterRestart('local', {
      zenHostKey: 'local',
      reloadPage: async () => {
        reloads += 1
        showPage(page([fleet, clock], 'v2'))
      },
    })
    expect(reloads).toBe(1)
    expect(keys).toEqual([`${GARDEN}/fleet`, `${GARDEN}/clock`])
    const run = useZenCustomRunStore.getState()
    expect(run.generation).toEqual({ [`${GARDEN}/fleet`]: 1, [`${GARDEN}/clock`]: 1 })
    expect(run.stopped).toEqual({})
  })

  it('a placement whose hash the new page changed is not remounted twice (the new hash remounts it)', async () => {
    showPage(page([custom('fleet'), custom('clock')]))
    stopZenWidget(`${GARDEN}/fleet`, 'not-responding')
    stopZenWidget(`${GARDEN}/gone`, 'failing')
    await relaunchZenAfterRestart('local', {
      zenHostKey: 'local',
      reloadPage: async () => showPage(page([custom('fleet', { hash: 'hash-2' }), custom('clock')], 'v2')),
    })
    const run = useZenCustomRunStore.getState()
    expect(run.generation).toEqual({ [`${GARDEN}/clock`]: 1 })
    // Every stop on the Garden goes, the gone placement's too.
    expect(run.stopped).toEqual({})
  })

  it('a remote room’s restart relaunches only the widgets that reach it, without re-reading the page', async () => {
    const fleet = custom('fleet', { sourceAddresses: [`fleet::${REMOTE}`] })
    const clock = custom('clock')
    showPage(page([fleet, clock]))
    stopZenWidget(`${GARDEN}/clock`, 'failing')
    let reloads = 0
    const keys = await relaunchZenAfterRestart(REMOTE, {
      zenHostKey: 'local',
      reloadPage: async () => {
        reloads += 1
      },
      serversOf: (w) => zenWidgetServers(w, () => ['local']),
    })
    expect(reloads).toBe(0)
    expect(keys).toEqual([`${GARDEN}/fleet`])
    const run = useZenCustomRunStore.getState()
    expect(run.generation).toEqual({ [`${GARDEN}/fleet`]: 1 })
    // The local widget's stop is not this restart's business.
    expect(run.stopped[`${GARDEN}/clock`]?.reason).toBe('failing')
  })
})

describe('end to end: hello → ready → the frame remounts once', () => {
  const bridge = {
    widgetId: 'fleet',
    caps: new Set<string>(),
    call: (verb: string) => {
      if (verb === 'gardens.current') return { id: GARDEN, name: 'Fleet Ops 2', index: 1 }
      throw new Error(`unexpected verb ${verb}`)
    },
  } as unknown as ZenWidgetBridge

  function Garden(): React.JSX.Element {
    const p = useZenConfigStore((s) => s.page)
    if (!p) throw new Error('no page')
    return (
      <div data-zen-root="">
        {p.widgets.map((d) => (
          <ZenCustomWidget key={d.id} decl={d} bridge={bridge} />
        ))}
      </div>
    )
  }

  it('same instance: nothing; new instance (booting, then ready): one page read and one remount', async () => {
    const fleet = custom('fleet', { caps: ['agents:read'] })
    showPage(page([fleet]))
    render(<Garden />)
    await vi.waitFor(() => expect(document.querySelector('[data-testid="zen-custom-frame"]')).not.toBeNull())
    expect(h.bundleGets).toBe(1)

    const timers = fakeTimers()
    const answers: ZenBootAnswer[] = [
      { ready: false, instanceId: 'inst-b' },
      { ready: true, instanceId: 'inst-b' },
    ]
    let reloads = 0
    const watch = createZenRestartWatch({
      boot: async () => {
        const a = answers.shift()
        if (!a) throw new Error('boot asked more often than the test expects')
        return a
      },
      relaunch: (hostKey) => {
        void relaunchZenAfterRestart(hostKey, {
          zenHostKey: 'local',
          reloadPage: async () => {
            reloads += 1
            showPage(page([fleet], 'v2'))
          },
        })
      },
      setTimer: timers.setTimer,
      clearTimer: timers.clearTimer,
    })

    // A socket blip: same daemon.
    await act(async () => {
      watch.note('local', 'inst-a')
      watch.note('local', 'inst-a')
      await settle()
    })
    expect(h.bundleGets).toBe(1)
    expect(reloads).toBe(0)

    // The daemon restarted and is still booting: nothing yet.
    await act(async () => {
      watch.note('local', 'inst-b')
      await settle()
    })
    expect(reloads).toBe(0)
    expect(h.bundleGets).toBe(1)

    // Ready: the page is read again and the frame remounts, once.
    await act(async () => {
      timers.fire()
      await settle()
    })
    await vi.waitFor(() => expect(h.bundleGets).toBe(2))
    expect(reloads).toBe(1)
    expect(document.querySelectorAll('[data-testid="zen-custom-frame"]').length).toBe(1)

    // More hellos from the new daemon change nothing.
    await act(async () => {
      watch.note('local', 'inst-b')
      await settle()
    })
    await act(async () => settle())
    expect(h.bundleGets).toBe(2)
    expect(reloads).toBe(1)
    watch.dispose()
  })
})
