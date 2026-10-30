// The LiveTransport contract (L2 transport spec v0 + addendum) against a
// fake socket: one stable object across reconnects, welcome first, send
// drops while not open, closed frames and close codes become onClose, a
// refusal rejects connect, late frames from a replaced socket are ignored.

import { describe, expect, it } from 'vitest'
import {
  CLOSE_CORE_OFF,
  LiveConnectError,
  closeFromCode,
  createLiveTransport,
  liveUrl,
  parseClose,
  type LiveClose,
  type LiveServerFrame,
  type LiveSocket,
} from './docs-live-transport'

class FakeSocket implements LiveSocket {
  readyState = 0
  sent: string[] = []
  closedWith: { code?: number; reason?: string } | null = null
  onopen: ((ev: unknown) => void) | null = null
  onmessage: ((ev: { data: unknown }) => void) | null = null
  onclose: ((ev: { code?: number; reason?: string }) => void) | null = null
  onerror: ((ev: unknown) => void) | null = null
  constructor(readonly url: string) {}

  send(data: string): void {
    if (this.readyState !== 1) throw new Error('send on a socket that is not open')
    this.sent.push(data)
  }
  close(code?: number, reason?: string): void {
    if (this.readyState >= 2) return
    this.closedWith = { code, reason }
    this.readyState = 2
    // As a browser does: the close event comes later.
    queueMicrotask(() => {
      this.readyState = 3
      this.onclose?.({ code: code ?? 1005 })
    })
  }
  /** The server side. */
  accept(): void {
    this.readyState = 1
    this.onopen?.({})
  }
  frame(f: unknown): void {
    this.onmessage?.({ data: JSON.stringify(f) })
  }
  drop(code = 1006): void {
    this.readyState = 3
    this.onclose?.({ code })
  }
  frames(): unknown[] {
    return this.sent.map((s) => JSON.parse(s))
  }
}

function harness() {
  const sockets: FakeSocket[] = []
  const urls: Array<number | undefined> = []
  const prevs: Array<number | undefined> = []
  /** At each dial: the states of the sockets dialled before it. */
  const priorStates: number[][] = []
  const t = createLiveTransport({
    url: async (resume, prev) => {
      urls.push(resume)
      prevs.push(prev)
      return `ws://x/cli/docs/live?resume=${resume ?? ''}`
    },
    open: async (url) => {
      priorStates.push(sockets.map((p) => p.readyState))
      const s = new FakeSocket(url)
      sockets.push(s)
      return s
    },
  })
  const frames: LiveServerFrame[] = []
  const closes: LiveClose[] = []
  t.onFrame((f) => frames.push(f))
  t.onClose((e) => closes.push(e))
  return { t, sockets, urls, prevs, frames, closes, priorStates }
}

const welcome = (over: Record<string, unknown> = {}) => ({
  type: 'welcome',
  root: 'source',
  version: 7,
  text: 'hello\n',
  client: 11,
  self: 'person:owner',
  rights: 'edit',
  ...over,
})

/** Let the transport's awaits run. */
const tick = () => new Promise((r) => setTimeout(r, 0))

async function connected(h: ReturnType<typeof harness>, resume?: number) {
  const p = h.t.connect({ resume, signal: new AbortController().signal })
  await tick()
  const s = h.sockets[h.sockets.length - 1]
  s.accept()
  s.frame(welcome(resume === undefined ? {} : { text: undefined, client: 12 + h.sockets.length }))
  const w = await p
  return { s, w }
}

describe('LiveTransport', () => {
  it('connect resolves with the welcome frame; welcome never reaches onFrame', async () => {
    const h = harness()
    const { s, w } = await connected(h)
    expect(w).toMatchObject({ root: 'source', version: 7, text: 'hello\n', client: 11, self: 'person:owner', rights: 'edit' })
    expect(h.t.open).toBe(true)
    s.frame({ type: 'change', event: { version: 8 } })
    s.frame({ type: 'roster_event', event: { kind: 'watching', count: 0 } })
    expect(h.frames.map((f) => f.type)).toEqual(['change', 'roster_event'])
    expect(h.urls).toEqual([undefined])
  })

  it('send drops (never queues) while not open', async () => {
    const h = harness()
    h.t.send({ type: 'since', op: 1, version: 0 }) // before any connect
    const p = h.t.connect({ signal: new AbortController().signal })
    await tick()
    const s = h.sockets[0]
    s.accept()
    h.t.send({ type: 'since', op: 2, version: 0 }) // open, but no welcome yet
    s.frame(welcome())
    await p
    h.t.send({ type: 'since', op: 3, version: 7 })
    expect(s.frames()).toEqual([{ type: 'since', op: 3, version: 7 }])
    s.frame({ type: 'closed', reason: 'cut_off', retry: true })
    h.t.send({ type: 'since', op: 4, version: 7 })
    expect(s.frames()).toHaveLength(1)
    expect(h.t.open).toBe(false)
  })

  it('a closed frame first rejects connect with its reason and retry (core off: no retry)', async () => {
    const h = harness()
    const p = h.t.connect({ signal: new AbortController().signal })
    await tick()
    const s = h.sockets[0]
    s.accept()
    s.frame({ type: 'closed', reason: 'closed', retry: false, code: 'core_off', core: 'markdown', message: 'the markdown core is off' })
    const err = await p.then(
      () => null,
      (e: unknown) => e,
    )
    expect(err).toBeInstanceOf(LiveConnectError)
    expect((err as LiveConnectError).close).toMatchObject({ reason: 'closed', retry: false, code: 'core_off', core: 'markdown' })
    expect(h.closes).toEqual([]) // a refused connect is the promise's, not onClose's
    expect(h.t.open).toBe(false)
  })

  it('a socket failing before welcome rejects as network (retry)', async () => {
    const h = harness()
    const p = h.t.connect({ signal: new AbortController().signal })
    await tick()
    h.sockets[0].drop(1006)
    await expect(p).rejects.toMatchObject({ close: { reason: 'network', retry: true } })
    const p2 = h.t.connect({ signal: new AbortController().signal })
    await tick()
    h.sockets[1].onerror?.({})
    await expect(p2).rejects.toMatchObject({ close: { reason: 'network', retry: true } })
  })

  it("the daemon's closed frame ends the socket once, with its fields", async () => {
    const h = harness()
    const { s } = await connected(h)
    s.frame({ type: 'closed', reason: 'closed', retry: false, code: 'core_off', core: 'markdown', version: 12, written: true })
    s.drop(CLOSE_CORE_OFF)
    expect(h.closes).toEqual([{ reason: 'closed', retry: false, code: 'core_off', core: 'markdown', version: 12, written: true }])
    s.frame({ type: 'change', event: { version: 13 } })
    expect(h.frames).toEqual([])
  })

  it('a close without a frame: 4010 is the core switched off, anything else network', async () => {
    const h = harness()
    const a = await connected(h)
    a.s.drop(CLOSE_CORE_OFF)
    const b = await connected(h, 7)
    b.s.drop(1006)
    expect(h.closes).toEqual([
      { reason: 'closed', retry: false, code: 'core_off' },
      { reason: 'network', retry: true },
    ])
  })

  it('one stable object across reconnects: listeners stay, a new client id, resume in the URL, old frames ignored', async () => {
    const h = harness()
    const first = await connected(h)
    first.s.drop()
    expect(h.closes).toHaveLength(1)
    const second = await connected(h, 7)
    expect(h.urls).toEqual([undefined, 7])
    expect(h.prevs).toEqual([undefined, first.w.client]) // the reconnect names its old connection
    expect(second.w.text).toBeUndefined()
    expect(second.w.client).not.toBe(first.w.client)
    first.s.frame({ type: 'change', event: { version: 99 } }) // a late frame from the replaced socket
    second.s.frame({ type: 'events', op: 1, events: [] })
    expect(h.frames.map((f) => f.type)).toEqual(['events'])
    h.t.send({ type: 'since', op: 2, version: 7 })
    expect(second.s.frames()).toEqual([{ type: 'since', op: 2, version: 7 }])
    expect(first.s.sent).toEqual([])
  })

  it('connect while a socket is open replaces it: the old one has closed before the new one dials', async () => {
    const h = harness()
    const first = await connected(h)
    const second = await connected(h, 7)
    expect(first.s.closedWith).toEqual({ code: 1000, reason: 'left' })
    expect(h.priorStates[1]).toEqual([3]) // never two live connections for one editor
    expect(h.closes).toEqual([])
    expect(h.t.open).toBe(true)
    second.s.frame({ type: 'ack', op: 1, version: 8, hunks: [] })
    expect(h.frames).toHaveLength(1)
  })

  it('a replaced socket that never reports its close holds the new dial for at most a second', async () => {
    const h = harness()
    const first = await connected(h)
    first.s.close = (code?: number, reason?: string) => {
      first.s.closedWith = { code, reason } // and never a close event
    }
    const started = Date.now()
    const p = h.t.connect({ resume: 7, signal: new AbortController().signal })
    while (h.sockets.length < 2) await new Promise((r) => setTimeout(r, 20))
    expect(Date.now() - started).toBeGreaterThanOrEqual(900)
    h.sockets[1].accept()
    h.sockets[1].frame(welcome({ text: undefined, client: 40 }))
    await expect(p).resolves.toMatchObject({ client: 40 })
    expect(first.s.closedWith).toEqual({ code: 1000, reason: 'left' })
  })

  it('an abort rejects the attempt and closes its socket', async () => {
    const h = harness()
    const ac = new AbortController()
    const p = h.t.connect({ signal: ac.signal })
    await tick()
    h.sockets[0].accept()
    ac.abort()
    await expect(p).rejects.toMatchObject({ name: 'AbortError' })
    expect(h.sockets[0].closedWith).toEqual({ code: 1000, reason: 'left' })
    const pre = new AbortController()
    pre.abort()
    await expect(h.t.connect({ signal: pre.signal })).rejects.toMatchObject({ name: 'AbortError' })
    expect(h.sockets).toHaveLength(1)
  })

  it('close() closes the socket, fires no onClose, and later sends drop', async () => {
    const h = harness()
    const { s } = await connected(h)
    h.t.close()
    expect(s.closedWith).toEqual({ code: 1000, reason: 'left' })
    s.drop(1000)
    expect(h.closes).toEqual([])
    h.t.send({ type: 'undo', op: 1 })
    expect(s.sent).toEqual([])
  })

  it('unsubscribing stops delivery; viewing and view frames go through send', async () => {
    const h = harness()
    const { s } = await connected(h)
    const seen: string[] = []
    const off = h.t.onFrame((f) => seen.push(f.type))
    s.frame({ type: 'undo_result', op: 1, outcome: 'nothing', steps: 0, version: 7 })
    off()
    s.frame({ type: 'snapshot', op: 2, text: 'x', version: 9 })
    expect(seen).toEqual(['undo_result'])
    h.t.send({ t: 'viewing', visible: true })
    h.t.send({ type: 'view', first: 1, last: 40 })
    expect(s.frames()).toEqual([
      { t: 'viewing', visible: true },
      { type: 'view', first: 1, last: 40 },
    ])
  })

  it('parses close frames strictly and builds the URL with the token last', () => {
    expect(parseClose({ type: 'closed', reason: 'teleported' })).toEqual({ reason: 'network', retry: true })
    expect(parseClose({ type: 'closed', reason: 'kicked', retry: false, error: { id: 'MPC-E111', message: 'm' } })).toEqual({
      reason: 'kicked',
      retry: false,
      error: { id: 'MPC-E111', message: 'm' },
    })
    expect(closeFromCode(undefined)).toEqual({ reason: 'network', retry: true })
    expect(parseClose({ type: 'closed', reason: 'released', retry: true })).toEqual({ reason: 'released', retry: false })
    expect(parseClose({ type: 'closed', reason: 'cut_off' })).toEqual({ reason: 'cut_off', retry: true })
    expect(liveUrl('ws://127.0.0.1:1', 't k', '/w/notes a.md', { resume: 5 })).toBe(
      'ws://127.0.0.1:1/cli/docs/live?doc=%2Fw%2Fnotes+a.md&root=source&resume=5&token=t+k',
    )
    expect(liveUrl('wss://h', 'tok', 'n.md', { workspace: 'ws' })).toBe('wss://h/cli/docs/live?doc=n.md&workspace=ws&root=source&token=tok')
    expect(liveUrl('wss://h', 'tok', 'n.md', { resume: 3, prev: 9007199254740991 })).toBe(
      'wss://h/cli/docs/live?doc=n.md&root=source&resume=3&prev=9007199254740991&token=tok',
    )
  })
})
