// Garden data SDK helpers in the frame runtime (contract-garden-data-v1
// §7.2, §7.4): k2.store.subscribe / tx() / putChunked / getChunked and
// k2.viewer(), run in the real k2-frame.js against a fake host that keeps
// one widget's keyspace in memory (the daemon's checks are Rust's tests).
import { createHash } from 'node:crypto'
import { runInContext, createContext } from 'node:vm'
import { MessageChannel, type MessagePort } from 'node:worker_threads'
import { afterEach, describe, expect, it } from 'vitest'
import { k2FrameScript } from './zen-custom-prelude'

type Listener = (e: unknown) => void

const ports: MessagePort[] = []

afterEach(() => {
  for (const p of ports.splice(0)) p.close()
})

interface Entry {
  key: string
  rev: number
  value: unknown
  updatedAt: number
  by: string
}

/** One widget's data, as the daemon would keep it (no grants: an owner-made widget). */
class FakeData {
  rev = 0
  entries = new Map<string, Entry>()
  staged = new Map<string, Array<Record<string, any>>>()
  parts: Array<Array<Record<string, any>>> = []
  heard: Array<{ verb: string; args: unknown[] }> = []
  nextTx = 1

  answer(verb: string, args: any[]): unknown {
    this.heard.push({ verb, args })
    switch (verb) {
      case 'viewer.get':
        return { kind: 'owner', name: 'Baden' }
      case 'store.get':
        return this.entries.get(args[0]) ?? null
      case 'store.list': {
        const prefix: string = args[0]?.prefix ?? ''
        const items = [...this.entries.values()].filter((e) => e.key.startsWith(prefix)).sort((a, b) => (a.key < b.key ? -1 : 1))
        return { items, next: null }
      }
      case 'store.put':
        this.apply([{ op: 'put', key: args[0], value: args[1] }])
        return { rev: this.rev }
      case 'store.txBegin': {
        const id = `tx${this.nextTx++}`
        this.staged.set(id, [])
        return { txId: id, expiresAt: 0 }
      }
      case 'store.txAdd': {
        const ops = args[1] as Array<Record<string, any>>
        if (ops.length > 64) throw new Error('part over 64 ops')
        // The daemon counts a patch's partial like a put's value (0.46.5).
        const bytes = ops.reduce((n, o) => n + (o.op === 'put' || o.op === 'patch' ? Buffer.byteLength(JSON.stringify(o.value)) : 0), 0)
        if (bytes > 60 * 1024) throw new Error(`part over 60 KiB: ${bytes}`)
        this.parts.push(ops)
        this.staged.get(args[0])!.push(...ops)
        return { ops: this.staged.get(args[0])!.length, bytes, expiresAt: 0 }
      }
      case 'store.txCommit': {
        const ops = this.staged.get(args[0])!
        this.staged.delete(args[0])
        this.apply(ops)
        return { rev: this.rev, ops: ops.length }
      }
      case 'store.txAbort':
        return { aborted: this.staged.delete(args[0]) }
      case 'store.batch': {
        const ops = args[0] as Array<Record<string, any>>
        this.apply(ops)
        return { rev: this.rev, ops: ops.length }
      }
      default:
        throw new Error(`fake host: ${verb}`)
    }
  }

  apply(ops: Array<Record<string, any>>): void {
    this.rev += 1
    for (const o of ops) {
      if (o.op === 'delete') this.entries.delete(o.key)
      else if (o.op === 'patch') {
        // Top-level fields only: the merge itself is Rust's (patch::merged).
        const merged: Record<string, unknown> = { ...((this.entries.get(o.key)?.value as Record<string, unknown>) ?? {}) }
        for (const [k, v] of Object.entries(o.value as Record<string, unknown>)) {
          if (v === null) delete merged[k]
          else merged[k] = v
        }
        this.entries.set(o.key, { key: o.key, rev: this.rev, value: merged, updatedAt: 1, by: 'you via board' })
      } else this.entries.set(o.key, { key: o.key, rev: this.rev, value: o.value, updatedAt: 1, by: 'you via board' })
    }
  }
}

function frame(): { k2: Record<string, any>; data: FakeData; heard: unknown[] } {
  const listeners = new Map<string, Listener[]>()
  const parent = {}
  const g: Record<string, unknown> = {
    addEventListener: (t: string, f: Listener) => listeners.set(t, [...(listeners.get(t) ?? []), f]),
    removeEventListener: (t: string, f: Listener) => listeners.set(t, (listeners.get(t) ?? []).filter((x) => x !== f)),
    setTimeout: () => 0,
    parent,
    navigator: { platform: 'MacIntel' },
    document: { documentElement: { style: { setProperty: () => undefined }, setAttribute: () => undefined } },
    console,
  }
  g.self = g
  runInContext(k2FrameScript(), createContext(g))
  const data = new FakeData()
  const heard: unknown[] = []
  const ch = new MessageChannel()
  ports.push(ch.port1, ch.port2)
  ch.port1.on('message', (m: any) => {
    heard.push(m)
    if (typeof m.id !== 'number') return
    try {
      ch.port1.postMessage({ id: m.id, ok: true, value: data.answer(m.verb, m.args) })
    } catch (e) {
      ch.port1.postMessage({ id: m.id, ok: false, error: { code: 'failed', message: String(e) } })
    }
  })
  for (const f of listeners.get('message') ?? []) {
    f({
      source: parent,
      data: {
        k2: 'hello',
        v: 1,
        caps: ['store:read', 'store:write'],
        features: ['zen-v1', 'garden-data-v1'],
        widget: { id: 'b1', name: 'Board', garden: 'g-1' },
        config: {},
        motion: { reduced: false },
      },
      ports: [ch.port2],
    })
  }
  return { k2: g.k2 as Record<string, any>, data, heard }
}

const sha = (s: string): string => createHash('sha256').update(s, 'utf8').digest('hex')

/** ~160 KB of text with multi-byte characters and JSON escapes in it. */
function bigText(): string {
  let s = ''
  for (let i = 0; s.length < 160_000; i++) s += `line ${i}: é “quotes” \\ \n 😀 data:image/png;base64,iVBORw0KGgo=`
  return s
}

describe('Garden data SDK helpers', () => {
  it('putChunked writes parts and a header in one transaction; getChunked joins and checks them', async () => {
    const { k2, data } = frame()
    const text = bigText()
    const r = await k2.store.putChunked('img/1', text)
    expect(r.ops).toBeGreaterThan(3)
    const head = data.entries.get('img/1')!.value as { chunks: number; bytes: number; sha256: string }
    expect(head).toEqual({ chunks: r.ops - 1, bytes: Buffer.byteLength(text, 'utf8'), sha256: sha(text) })
    expect(data.heard.filter((h) => h.verb === 'store.txCommit')).toHaveLength(1)
    expect(data.parts.every((p) => p.length <= 64)).toBe(true)
    await expect(k2.store.getChunked('img/1')).resolves.toBe(text)
    await expect(k2.store.getChunked('img/none')).resolves.toBeNull()
  })

  it('a part that doesn’t match its header fails with chunk_mismatch', async () => {
    const { k2, data } = frame()
    await k2.store.putChunked('img/1', bigText())
    const p = data.entries.get('img/1/~1')!
    data.entries.set('img/1/~1', { ...p, value: { part: 1, text: 'tampered' } })
    const err = await k2.store.getChunked('img/1').catch((e: unknown) => e)
    expect(err).toMatchObject({ name: 'K2Error', code: 'failed', reason: 'chunk_mismatch' })
    data.entries.delete('img/1/~2')
    await expect(k2.store.getChunked('img/1')).rejects.toMatchObject({ reason: 'chunk_mismatch' })
  })

  it('a shorter value deletes the old parts past its end; over 1 MiB is too_large before any call', async () => {
    const { k2, data } = frame()
    await k2.store.putChunked('doc', bigText())
    expect([...data.entries.keys()].filter((k) => k.startsWith('doc/~')).length).toBeGreaterThan(2)
    await k2.store.putChunked('doc', 'short', { ifRev: data.entries.get('doc')!.rev })
    expect([...data.entries.keys()].filter((k) => k.startsWith('doc/~'))).toEqual(['doc/~0'])
    await expect(k2.store.getChunked('doc')).resolves.toBe('short')
    const before = data.heard.length
    await expect(k2.store.putChunked('huge', 'x'.repeat(1024 * 1024 + 1))).rejects.toMatchObject({ code: 'too_large' })
    expect(data.heard.length).toBe(before)
  })

  it('tx() stages parts in order and commits them as one', async () => {
    const { k2, data } = frame()
    const tx = k2.store.tx()
    const ops = Array.from({ length: 130 }, (_, i) => ({ op: 'put', key: `obj/${i}`, value: { i } }))
    await tx.add(ops)
    expect(data.parts.map((p) => p.length)).toEqual([64, 64, 2])
    expect(data.entries.size).toBe(0)
    const r = await tx.commit()
    expect(r).toEqual({ rev: 1, ops: 130 })
    expect(data.entries.size).toBe(130)
    await expect(k2.store.tx().abort()).resolves.toEqual({ aborted: false })
  })

  it('0.46.5: batch and tx() carry patch ops as they are; a patch partial counts toward a part’s 60 KiB', async () => {
    const { k2, data } = frame()
    await k2.store.put('obj/1', { t: 'a', x: 1, col: 'todo' })
    const ops = [
      { op: 'patch', key: 'obj/1', value: { x: 2, t: null }, ifRev: 1 },
      { op: 'put', key: 'edge/1', value: { from: 'obj/1' } },
      { op: 'delete', key: 'edge/0' },
    ]
    await expect(k2.store.batch(ops)).resolves.toEqual({ rev: 2, ops: 3 })
    expect(data.heard.find((h) => h.verb === 'store.batch')?.args).toEqual([ops])
    expect(data.entries.get('obj/1')?.value).toEqual({ x: 2, col: 'todo' })
    // Three ~25 KiB partials: two fit in one part, the third starts another.
    const big = 'y'.repeat(25 * 1024)
    const tx = k2.store.tx()
    await tx.add([0, 1, 2].map((i) => ({ op: 'patch', key: `obj/${i + 10}`, value: { note: big } })))
    expect(data.parts.slice(-2).map((p) => p.length)).toEqual([2, 1])
    expect(data.parts.slice(-2).flat().every((o) => o.op === 'patch')).toBe(true)
    await expect(tx.commit()).resolves.toEqual({ rev: 3, ops: 3 })
    expect(data.entries.get('obj/12')?.value).toEqual({ note: big })
  })

  it('k2.viewer() and k2.viewer.get() name the person; subscribe rides store.changes', async () => {
    const { k2, heard } = frame()
    await expect(k2.viewer()).resolves.toEqual({ kind: 'owner', name: 'Baden' })
    await expect(k2.viewer.get()).resolves.toEqual({ kind: 'owner', name: 'Baden' })
    expect(Object.isFrozen(k2.store)).toBe(true)
    const off = k2.store.subscribe({ prefix: 'obj/', since: 4 }, () => undefined)
    await new Promise((r) => setTimeout(r, 15))
    expect(heard).toContainEqual({ sub: expect.any(Number), verb: 'store.changes', args: [{ prefix: 'obj/', since: 4 }] })
    off()
    expect(k2.can('store.put')).toBe(true)
    expect(k2.can('thread.post')).toBe(false)
  })
})
