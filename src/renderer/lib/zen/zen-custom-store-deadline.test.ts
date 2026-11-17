// A Garden widget's snapshot read always gets an answer (Rosson 2026-11-16:
// the Compute Garden's Compute load tile stopped following its nodes). The
// tile chains its next 30 s poll on each k2.compute.nodes() answer, so a
// read the daemon never answers stopped it for good while the terminal's
// `k2 compute nodes` moved on. The store answers such a read within
// ZEN_STORE_READ_DEADLINE_MS (`unreachable`, retryable) and gives the
// request up; writes never time out (a retry could repeat one).
// Fake timers only: no real waits.
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  createZenCustomStore,
  zenStoreErrorFrom,
  ZEN_STORE_DEADLINE_VERBS,
  ZEN_STORE_READ_DEADLINE_MS,
  ZEN_STORE_VERBS,
  type ZenCustomStoreDeps,
  type ZenStoreError,
} from './zen-custom-store'

/* eslint-disable @typescript-eslint/no-explicit-any */
type Any = any

const DEADLINE = 20_000
const never = (): Promise<unknown> => new Promise(() => undefined)

interface Daemon {
  deps: ZenCustomStoreDeps
  calls: Array<{ verb: string; opts: unknown }>
}

/** The daemon door: `answers` in turn per call (a function, or `never`). */
function daemon(answers: Array<() => Promise<unknown>>): Daemon {
  const d: Daemon = { calls: [] } as unknown as Daemon
  d.deps = {
    call: (verb, _args, opts) => {
      d.calls.push({ verb, opts: opts ?? null })
      const next = answers.shift()
      if (!next) throw new Error(`no answer queued for ${verb}`)
      return next()
    },
    onChanged: () => () => undefined,
    space: 'data:k2:canvas',
    onStale: () => {
      throw new Error('no stale reload expected')
    },
    setTimer: (fn, ms) => setTimeout(fn, ms),
    clearTimer: (t) => clearTimeout(t as ReturnType<typeof setTimeout>),
    now: () => Date.now(),
  }
  return d
}

beforeEach(() => {
  vi.useFakeTimers()
})
afterEach(() => {
  vi.useRealTimers()
})

describe('the store: a snapshot read always gets an answer', () => {
  it('answers a compute.nodes read the daemon never answers with unreachable at the deadline, and gives the request up', async () => {
    const d = daemon([never])
    const s = createZenCustomStore(d.deps)
    let outcome: { code: string; message: string } | 'pending' = 'pending'
    s.call('compute.nodes', []).then(
      () => {
        outcome = { code: 'answered', message: '' }
      },
      (e: ZenStoreError) => {
        outcome = e.wire
      },
    )
    await vi.advanceTimersByTimeAsync(DEADLINE - 1)
    expect(outcome).toBe('pending')
    await vi.advanceTimersByTimeAsync(1)
    expect(outcome).toEqual({ code: 'unreachable', message: 'no answer from K2 in 20 s' })
    // The request itself is given up at the same deadline (an aborted fetch).
    expect(d.calls).toEqual([{ verb: 'compute.nodes', opts: { timeoutMs: DEADLINE } }])
  })

  it('passes an answer in time through, and drops one that comes after the deadline', async () => {
    let late: (v: unknown) => void = () => undefined
    const d = daemon([
      () => Promise.resolve({ at: 1, nodes: [] }),
      () =>
        new Promise((r) => {
          late = r
        }),
    ])
    const s = createZenCustomStore(d.deps)
    await expect(s.call('compute.nodes', [])).resolves.toEqual({ at: 1, nodes: [] })
    const second = s.call('compute.nodes', [])
    const seen = expect(second).rejects.toMatchObject({ wire: { code: 'unreachable' } })
    await vi.advanceTimersByTimeAsync(DEADLINE)
    await seen
    late({ at: 2, nodes: [] })
    await vi.advanceTimersByTimeAsync(0)
    expect(vi.getTimerCount()).toBe(0)
  })

  it('never puts a deadline on a write', async () => {
    const d = daemon([never])
    const s = createZenCustomStore(d.deps)
    let settled = false
    s.call('store.put', ['k', 1]).finally(() => {
      settled = true
    })
    await vi.advanceTimersByTimeAsync(10 * DEADLINE)
    expect(settled).toBe(false)
    expect(d.calls).toEqual([{ verb: 'store.put', opts: null }])
  })

  it('names only garden-local reads, with a 20 s deadline', () => {
    expect(ZEN_STORE_READ_DEADLINE_MS).toBe(DEADLINE)
    for (const v of ZEN_STORE_DEADLINE_VERBS) expect(ZEN_STORE_VERBS.has(v), v).toBe(true)
    expect(ZEN_STORE_DEADLINE_VERBS.has('compute.nodes')).toBe(true)
    for (const w of ['store.put', 'store.patch', 'store.delete', 'store.batch', 'store.txCommit', 'save.set', 'tickets.create', 'thread.mute']) {
      expect(ZEN_STORE_DEADLINE_VERBS.has(w), w).toBe(false)
    }
  })

  it('reads a daemon client time out as unreachable', () => {
    const t = Object.assign(new Error('no answer from the server in 20 s (zen/call)'), { name: 'CliTimeoutError' })
    expect(zenStoreErrorFrom(t).wire).toEqual({ code: 'unreachable', message: 'no answer from the server in 20 s (zen/call)' })
  })
})

// The shipped tile, k2:canvas@1's Compute load, on the real store.
const { JSDOM } = createRequire(import.meta.url)('jsdom') as {
  JSDOM: new (html: string, opts: { runScripts: 'outside-only'; pretendToBeVisual: boolean }) => { window: unknown }
}
const DIR = resolve(__dirname, '../../../../crates/k2-core/src/zen/builtin_widgets/canvas-1')
const HTML = readFileSync(resolve(DIR, 'index.html'), 'utf8')
const SCRIPTS = [...HTML.matchAll(/<script src="([^"]+)"><\/script>/g)].map((m) => readFileSync(resolve(DIR, m[1]), 'utf8'))

function node(name: string, f: Any): Any {
  return Object.assign(
    {
      name, os: 'linux', arch: 'x86_64', version: '0.46.13', online: true, state: 'active', why: null,
      slots: 2, running: 0, queued: 0, queueFrom: 'server',
      cpu: { freeMillis: 24000, totalMillis: 24000, idleMillis: 24000 }, mem: { freeBytes: 20 * 2 ** 30, totalBytes: 20 * 2 ** 30 },
      load: null, lastSeenAt: 1, shared: false, servers: null, pinnedOnly: false, who: [], jobs: [], queueError: null,
    },
    f,
  )
}

describe('k2:canvas@1 Compute load on the store', () => {
  it('keeps polling after a read the daemon never answers, and shows the new availability', async () => {
    const at = Math.floor(Date.now() / 1000)
    const d = daemon([
      never,
      () => Promise.resolve({ at: at + 50, controller: 'this.k2.dev', nodes: [node('test-node', { running: 1, cpu: { freeMillis: 20000, totalMillis: 24000, idleMillis: 24000 } })] }),
    ])
    const store = createZenCustomStore(d.deps)
    const dom = new JSDOM(HTML.replace(/<script[^>]*><\/script>/g, '').replace(/<link[^>]*>/g, ''), { runScripts: 'outside-only', pretendToBeVisual: true })
    const win = dom.window as Any
    // The frame's timers are the test's fake ones.
    win.setTimeout = (fn: () => void, ms: number) => setTimeout(fn, ms)
    win.clearTimeout = (t: ReturnType<typeof setTimeout>) => clearTimeout(t)
    win.setInterval = (fn: () => void, ms: number) => setInterval(fn, ms)
    win.clearInterval = (t: ReturnType<typeof setInterval>) => clearInterval(t)
    win.k2 = {
      connected: Promise.resolve(),
      config: { tiles: 'compute' },
      can: (v: string) => v === 'compute.nodes',
      ready: () => undefined,
      onVisible: () => () => undefined,
      save: { get: async () => null, set: async () => null },
      canvas: { setHitRegions: async () => null },
      compute: {
        // As k2-frame.js hands a refusal to the widget: a K2Error {code, message}.
        nodes: () =>
          store.call('compute.nodes', []).then(undefined, (e: ZenStoreError) => {
            throw Object.assign(new Error(e.wire.message), e.wire)
          }),
      },
    }
    for (const src of SCRIPTS) win.eval(src)
    await vi.advanceTimersByTimeAsync(0)
    const doc = win.document
    expect(d.calls).toHaveLength(1)
    expect(doc.querySelector('.cl-empty').textContent).toBe('Reading the compute nodes…')

    // The first read never comes back: the tile says so at the deadline...
    await vi.advanceTimersByTimeAsync(DEADLINE)
    expect(doc.querySelector('.cv-tile-note').textContent).toBe('K2 didn’t read the nodes (unreachable: no answer from K2 in 20 s).')
    // ...and reads again 30 s later, drawing what the nodes are now.
    await vi.advanceTimersByTimeAsync(30_000)
    expect(d.calls).toHaveLength(2)
    const card = doc.querySelector('.cl-node')
    expect(card.querySelector('.cl-name').textContent).toBe('test-node')
    expect(card.querySelector('.cl-count').textContent).toBe('1/2')
    expect(doc.querySelector('.cv-tile-note').hidden).toBe(true)
  })
})
