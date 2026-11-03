// contract-gardens-phone rev 2 §B.3: k2.view, k2.viewport, k2.setHeight,
// k2.onVisible and auto height, run in the real k2-frame.js text in a fresh
// realm with a fake window. A desktop's hello has neither `view` nor `height`:
// none of this may ever post there.

import { createContext, runInContext } from 'node:vm'
import { MessageChannel, type MessagePort } from 'node:worker_threads'
import { afterEach, describe, expect, it } from 'vitest'

import { k2FrameScript } from './zen-custom-prelude'

type Listener = (e: unknown) => void

interface FakeBody {
  height: number
  children: Array<{ scrollHeight: number }>
}

interface Realm {
  k2: Record<string, any>
  fire(type: string, e: unknown): void
  parent: object
  runTimers(): void
  /** What the runtime watched: observers made and load listeners. */
  watched: string[]
  observers: Array<() => void>
}

const ports: MessagePort[] = []

function realm(body: FakeBody | null = null): Realm {
  const listeners = new Map<string, Listener[]>()
  const timers: Array<() => void> = []
  const parent = {}
  const watched: string[] = []
  const observers: Array<() => void> = []
  const observer = (name: string) =>
    class {
      constructor(cb: () => void) {
        observers.push(cb)
      }
      observe(): void {
        watched.push(name)
      }
    }
  const g: Record<string, unknown> = {
    addEventListener: (t: string, f: Listener) => listeners.set(t, [...(listeners.get(t) ?? []), f]),
    removeEventListener: (t: string, f: Listener) => listeners.set(t, (listeners.get(t) ?? []).filter((x) => x !== f)),
    setTimeout: (f: () => void) => {
      timers.push(f)
      return timers.length
    },
    parent,
    navigator: { platform: 'iPhone' },
    MutationObserver: observer('mutation'),
    ResizeObserver: observer('resize'),
    document: {
      documentElement: { style: { setProperty: () => {} }, setAttribute: () => {}, getAttribute: () => null },
      body: body && { getBoundingClientRect: () => ({ height: body.height }), children: body.children },
      addEventListener: (t: string) => watched.push(`document:${t}`),
    },
    console,
  }
  g.self = g
  const ctx = createContext(g)
  runInContext(k2FrameScript(), ctx)
  return {
    k2: g.k2 as Record<string, any>,
    fire: (type, e) => {
      for (const f of listeners.get(type) ?? []) f(e)
    },
    parent,
    runTimers: () => {
      for (const t of timers.splice(0)) t()
    },
    watched,
    observers,
  }
}

function connect(r: Realm, over: Record<string, unknown> = {}): { host: MessagePort; heard: any[] } {
  const ch = new MessageChannel()
  ports.push(ch.port1, ch.port2)
  const heard: any[] = []
  ch.port1.on('message', (m) => heard.push(m))
  r.fire('message', {
    source: r.parent,
    data: {
      k2: 'hello',
      v: 1,
      caps: [],
      features: [],
      widget: { id: 'b1', name: 'Board', garden: 'g-test0001' },
      config: {},
      motion: { reduced: false },
      ...over,
    },
    ports: [ch.port2],
  })
  return { host: ch.port1, heard }
}

const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 15))
const sizes = (heard: any[]): number[] => heard.filter((m) => m && m.size).map((m) => m.size.height)
const PHONE = { view: 'phone', height: 'fill' }

afterEach(() => {
  for (const p of ports.splice(0)) p.close()
})

describe('on a desktop', () => {
  it('reads as desktop and never posts a size', async () => {
    const r = realm({ height: 500, children: [{ scrollHeight: 900 }] })
    expect(r.k2.view).toBe('desktop')
    expect(r.k2.viewport).toBeNull()
    const { heard } = connect(r)
    await r.k2.connected
    r.k2.setHeight(300)
    r.runTimers()
    await tick()
    expect(r.k2.view).toBe('desktop')
    expect(r.k2.viewport).toBeNull()
    expect(sizes(heard)).toEqual([])
    expect(r.watched).toEqual([])
    expect(r.observers).toEqual([])
  })

  it('drops a height asked for before the hello', async () => {
    const r = realm()
    r.k2.setHeight(300)
    const { heard } = connect(r)
    await r.k2.connected
    await tick()
    expect(sizes(heard)).toEqual([])
  })

  it('keeps a hello with no view exactly as it was', async () => {
    const r = realm()
    connect(r)
    const hello = await r.k2.connected
    expect(Object.keys(hello).sort()).toEqual(['caps', 'config', 'features', 'motion', 'widget'])
  })
})

describe('on a phone', () => {
  it('says phone and reads the viewport from the hello and the port', async () => {
    const r = realm()
    const { host } = connect(r, { ...PHONE, viewport: { width: 390, height: 720 } })
    await r.k2.connected
    expect(r.k2.view).toBe('phone')
    expect({ ...r.k2.viewport }).toEqual({ width: 390, height: 720 })
    host.postMessage({ viewport: { width: 720, height: 390 } })
    await tick()
    expect({ ...r.k2.viewport }).toEqual({ width: 720, height: 390 })
    host.postMessage({ viewport: { width: 'wide', height: 1 } })
    await tick()
    expect(r.k2.viewport).toBeNull()
  })

  it('has no viewport when the phone sends none', async () => {
    const r = realm()
    connect(r, PHONE)
    await r.k2.connected
    expect(r.k2.viewport).toBeNull()
  })

  it('posts setHeight at most 4 a second, newest wins, and skips a repeat', async () => {
    const r = realm()
    const { heard } = connect(r, PHONE)
    await r.k2.connected
    r.k2.setHeight(300.2)
    await tick()
    expect(sizes(heard)).toEqual([301])
    r.k2.setHeight(400)
    r.k2.setHeight(500)
    await tick()
    expect(sizes(heard)).toEqual([301])
    r.runTimers()
    await tick()
    expect(sizes(heard)).toEqual([301, 500])
    r.k2.setHeight(500)
    r.runTimers()
    await tick()
    expect(sizes(heard)).toEqual([301, 500])
  })

  it('ignores a height that is not a positive number', async () => {
    const r = realm()
    const { heard } = connect(r, PHONE)
    await r.k2.connected
    for (const bad of [0, -5, Number.NaN, Number.POSITIVE_INFINITY, '300', null, undefined]) r.k2.setHeight(bad)
    r.runTimers()
    await tick()
    expect(sizes(heard)).toEqual([])
  })

  it('posts a height asked for before the hello', async () => {
    const r = realm()
    r.k2.setHeight(250)
    const { heard } = connect(r, PHONE)
    await r.k2.connected
    await tick()
    expect(sizes(heard)).toEqual([250])
  })

  it('tells onVisible only on a change, and an unsubscribe stops it', async () => {
    const r = realm()
    const { host } = connect(r, PHONE)
    await r.k2.connected
    const seen: boolean[] = []
    const off = r.k2.onVisible((v: boolean) => seen.push(v))
    expect(seen).toEqual([])
    host.postMessage({ visible: true })
    await tick()
    expect(seen).toEqual([])
    host.postMessage({ visible: false })
    await tick()
    host.postMessage({ visible: false })
    await tick()
    host.postMessage({ visible: true })
    await tick()
    expect(seen).toEqual([false, true])
    off()
    host.postMessage({ visible: false })
    await tick()
    expect(seen).toEqual([false, true])
  })

  it('a listener that throws stops neither the others nor the pongs', async () => {
    const r = realm()
    const { host, heard } = connect(r, PHONE)
    await r.k2.connected
    const seen: boolean[] = []
    r.k2.onVisible(() => {
      throw new Error('a widget bug')
    })
    r.k2.onVisible((v: boolean) => seen.push(v))
    host.postMessage({ visible: false })
    await tick()
    expect(seen).toEqual([false])
    host.postMessage({ ping: 7 })
    await tick()
    expect(heard).toContainEqual({ pong: 7 })
  })

  it('answers a ping while hidden', async () => {
    const r = realm()
    const { host, heard } = connect(r, PHONE)
    await r.k2.connected
    host.postMessage({ visible: false })
    await tick()
    host.postMessage({ ping: 3 })
    await tick()
    expect(heard).toContainEqual({ pong: 3 })
  })
})

describe('auto height', () => {
  const AUTO = { view: 'phone', height: 'auto' }

  it('reports the larger of the body and each top-level scrollHeight', async () => {
    const body: FakeBody = { height: 300, children: [{ scrollHeight: 120 }, { scrollHeight: 640.5 }] }
    const r = realm(body)
    const { heard } = connect(r, AUTO)
    await r.k2.connected
    expect(r.watched).toEqual(['mutation', 'resize', 'document:load'])
    r.runTimers()
    await tick()
    expect(sizes(heard)).toEqual([641])
    // The page changes: one measure, and the new height after the gap.
    body.children[1].scrollHeight = 200
    r.observers[0]()
    r.observers[1]()
    r.runTimers()
    r.runTimers()
    await tick()
    expect(sizes(heard).at(-1)).toBe(300)
  })

  it('never reports for fill or a px height', async () => {
    for (const height of ['fill', 420]) {
      const r = realm({ height: 300, children: [{ scrollHeight: 900 }] })
      const { heard } = connect(r, { view: 'phone', height })
      await r.k2.connected
      r.runTimers()
      await tick()
      expect(r.watched).toEqual([])
      expect(sizes(heard)).toEqual([])
    }
  })

  it('steps aside once the widget sizes itself', async () => {
    const r = realm({ height: 300, children: [{ scrollHeight: 900 }] })
    const { heard } = connect(r, AUTO)
    await r.k2.connected
    r.k2.setHeight(444)
    r.runTimers()
    await tick()
    expect(sizes(heard)).toEqual([444])
    r.observers[0]()
    r.runTimers()
    r.runTimers()
    await tick()
    expect(sizes(heard)).toEqual([444])
  })
})
