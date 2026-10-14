// @vitest-environment jsdom
// The board-lite widget example (crates/k2-core/src/zen/examples/board-lite;
// contract-garden-data-v1 §11.4, §12.3/§12.4) against a fake `k2` with the
// Garden data SDK shape (contract §7.2): store.list/get/put/delete/subscribe
// and viewer(). Proves Baden's Board flow at the widget's edge:
//   - one key per object under the placement's config.prefix;
//   - an agent's write shows live (no reload), labelled with its handle;
//   - field-level writes: a conflict re-reads and re-applies only the
//     fields this gesture changed (the agent's text survives a move);
//   - every write carries a batch id; a resync redraws from the items;
//   - author_unknown / not_granted show what to do; bad values don't break it.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'

const DIR = resolve(__dirname, '../../../../crates/k2-core/src/zen/examples/board-lite')
const read = (p: string): string => readFileSync(resolve(DIR, p), 'utf8')

type Json = unknown
interface Stored {
  rev: number
  value: Json
  by: string
}
type Change = { key: string; rev: number; value: Json | null; by: string }

class K2Error extends Error {
  code: string
  constructor(code: string) {
    super(code)
    this.code = code
  }
}

function fakeK2(opts: { prefix?: string; caps?: string[]; listError?: string } = {}) {
  const data = new Map<string, Stored>()
  let head = 0
  const subs: Array<{ prefix: string; cb: (c: Change[] | { resync: true; items: unknown[] }) => void; onError?: (e: unknown) => void }> = []
  const puts: Array<{ key: string; value: Record<string, unknown>; ifRev: number | undefined }> = []
  let ready = 0
  let failNextPut: string | null = null
  const push = (changes: Change[]): void => {
    for (const s of subs) {
      const mine = changes.filter((c) => c.key.startsWith(s.prefix))
      if (mine.length) s.cb(mine)
    }
  }
  const write = (key: string, value: Json | null, by: string): number => {
    head += 1
    if (value === null) data.delete(key)
    else data.set(key, { rev: head, value, by })
    push([{ key, rev: head, value, by }])
    return head
  }
  const k2 = {
    connected: Promise.resolve(),
    config: opts.prefix === undefined ? {} : { prefix: opts.prefix },
    can: (verb: string) => (opts.caps ?? ['store.list', 'store.put', 'store.get', 'store.delete', 'store.subscribe']).includes(verb),
    ready: () => {
      ready += 1
    },
    viewer: async () => ({ kind: 'owner', name: 'Rosson' }),
    store: {
      list: async (o: { prefix: string; after?: string; limit?: number }) => {
        if (opts.listError) throw new K2Error(opts.listError)
        const keys = [...data.keys()].filter((k) => k.startsWith(o.prefix) && (!o.after || k > o.after)).sort()
        const page = keys.slice(0, 2) // small pages: the widget must follow `next`
        return {
          items: page.map((k) => ({ key: k, rev: data.get(k)!.rev, value: data.get(k)!.value, updatedAt: 1, by: data.get(k)!.by })),
          next: keys.length > 2 ? page[page.length - 1] : null,
        }
      },
      get: async (key: string) => {
        const s = data.get(key)
        return s ? { key, rev: s.rev, value: s.value, updatedAt: 1, by: s.by } : null
      },
      put: async (key: string, value: Record<string, unknown>, o: { ifRev?: number } = {}) => {
        puts.push({ key, value, ifRev: o.ifRev })
        if (failNextPut) {
          const c = failNextPut
          failNextPut = null
          throw new K2Error(c)
        }
        const cur = data.get(key)
        if (o.ifRev !== undefined && (cur?.rev ?? 0) !== o.ifRev) throw new K2Error('conflict')
        return { rev: write(key, value, 'you') }
      },
      delete: async (key: string, o: { ifRev?: number } = {}) => {
        const cur = data.get(key)
        if (o.ifRev !== undefined && (cur?.rev ?? 0) !== o.ifRev) throw new K2Error('conflict')
        if (!cur) return { rev: 0, deleted: false }
        return { rev: write(key, null, 'you'), deleted: true }
      },
      subscribe: (prefix: string, cb: (c: Change[] | { resync: true; items: unknown[] }) => void, onError?: (e: unknown) => void) => {
        subs.push({ prefix, cb, onError })
        return () => undefined
      },
    },
  }
  return {
    k2,
    data,
    puts,
    subs,
    ready: () => ready,
    /** Someone else (an agent, another window) writes. */
    agentWrite: (key: string, value: Json | null, by = 'agent:board-keeper') => write(key, value, by),
    /** Write without a push: data that was there before the widget ran, or
     *  a write the widget hasn't heard about yet. Answers the new rev. */
    seed: (key: string, value: Json, by = 'you') => {
      head += 1
      data.set(key, { rev: head, value, by })
      return head
    },
    failNextPut: (code: string) => {
      failNextPut = code
    },
    resync: () => {
      const items = [...data.entries()].map(([key, s]) => ({ key, rev: s.rev, value: s.value, by: s.by }))
      for (const s of subs) s.cb({ resync: true, items })
    },
  }
}

const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 0))
async function until(what: string, ok: () => boolean): Promise<void> {
  for (let i = 0; i < 200; i++) {
    if (ok()) return
    await tick()
  }
  throw new Error(`timed out waiting for: ${what}`)
}

function mount(f: ReturnType<typeof fakeK2>): void {
  const html = read('index.html')
  document.body.innerHTML = html.slice(html.indexOf('<body>') + 6, html.indexOf('<script'))
  new Function('k2', 'document', read('board-lite.js'))(f.k2, document)
}

function sticky(key: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`.sticky[data-key="${key}"]`)
}

const P = 'project/7/board/1/'

afterEach(() => {
  document.body.innerHTML = ''
})

describe('board-lite example', () => {
  it('manifest asks for the store caps only', () => {
    const m = JSON.parse(read('manifest.json'))
    expect(m.caps).toEqual(['store:read', 'store:write'])
    // Code, not comments: no innerHTML, inline handlers, network or storage.
    const code = read('board-lite.js')
      .split('\n')
      .filter((l) => !l.trim().startsWith('//'))
      .join('\n')
    expect(code).not.toMatch(/innerHTML|onclick=|fetch\(|localStorage/)
  })

  it('draws every note under the prefix (following next), then calls ready', async () => {
    const f = fakeK2({ prefix: P })
    f.seed(`${P}obj/a`, { id: 'a', text: 'one', x: 10, y: 20, color: 'blue' })
    f.seed(`${P}obj/b`, { id: 'b', text: 'two', x: 30, y: 40 })
    f.seed(`${P}obj/c`, { id: 'c', text: 'three', x: 50, y: 60 }, 'agent:board-keeper')
    f.seed('project/8/board/1/obj/z', { text: 'not mine' })
    mount(f)
    await until('ready', () => f.ready() === 1)
    expect(document.getElementById('count')?.textContent).toBe('3 notes')
    expect(sticky(`${P}obj/a`)?.querySelector('.text')?.textContent).toBe('one')
    expect(sticky(`${P}obj/a`)?.dataset.color).toBe('blue')
    expect(sticky(`${P}obj/a`)?.style.left).toBe('10px')
    expect(sticky(`${P}obj/c`)?.querySelector('.by')?.textContent).toBe('board-keeper')
    expect(sticky(`${P}obj/a`)?.querySelector('.by')?.textContent).toBe('Rosson')
    expect(document.querySelector('[data-key="project/8/board/1/obj/z"]')).toBeNull()
    expect(f.subs.map((s) => s.prefix)).toEqual([`${P}obj/`])
    expect((document.getElementById('add') as HTMLButtonElement).disabled).toBe(false)
  })

  it("an agent's write shows live with no reload, labelled with its handle; a delete removes it", async () => {
    const f = fakeK2({ prefix: P })
    mount(f)
    await until('ready', () => f.ready() === 1)
    f.agentWrite(`${P}obj/9`, { id: '9', text: 'from the keeper', x: 5, y: 5 })
    expect(sticky(`${P}obj/9`)?.querySelector('.text')?.textContent).toBe('from the keeper')
    expect(sticky(`${P}obj/9`)?.querySelector('.by')?.textContent).toBe('board-keeper')
    f.agentWrite(`${P}obj/9`, { id: '9', text: 'from the keeper', x: 200, y: 40 })
    expect(sticky(`${P}obj/9`)?.style.left).toBe('200px')
    f.agentWrite(`${P}obj/9`, null)
    expect(sticky(`${P}obj/9`)).toBeNull()
    expect(document.getElementById('count')?.textContent).toBe('0 notes')
  })

  it('+ Note creates one key per object with ifRev 0 and a batch id', async () => {
    const f = fakeK2({ prefix: P })
    mount(f)
    await until('ready', () => f.ready() === 1)
    ;(document.getElementById('add') as HTMLButtonElement).click()
    await until('a put', () => f.puts.length === 1)
    const put = f.puts[0]
    expect(put.key.startsWith(`${P}obj/`)).toBe(true)
    expect(put.ifRev).toBe(0)
    expect(put.value).toMatchObject({ text: 'New note', color: 'yellow' })
    expect(typeof put.value.batch).toBe('string')
    expect(put.value.id).toBe(put.key.slice(`${P}obj/`.length))
    await until('drawn', () => sticky(put.key) !== null)
    expect(sticky(put.key)?.querySelector('.by')?.textContent).toBe('Rosson')
  })

  it('field-level: an edit that hits a conflict re-reads and re-applies only its own field', async () => {
    const f = fakeK2({ prefix: '' })
    f.seed('obj/n', { id: 'n', text: 'old text', x: 10, y: 10 })
    mount(f)
    await until('ready', () => f.ready() === 1)
    // The agent moves the note while the widget still holds the old rev
    // (no push reaches the widget for it: a write it hasn't seen yet).
    const seen = f.data.get('obj/n')!.rev
    const moved = f.seed('obj/n', { id: 'n', text: 'old text', x: 300, y: 90 }, 'agent:board-keeper')
    const el = sticky('obj/n') as HTMLElement
    el.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }))
    const box = el.querySelector('textarea') as HTMLTextAreaElement
    box.value = 'new text'
    box.dispatchEvent(new FocusEvent('blur'))
    await until('two puts', () => f.puts.length === 2)
    expect(f.puts[0].ifRev).toBe(seen)
    expect(f.puts[1].ifRev).toBe(moved)
    expect(f.puts[1].value).toMatchObject({ text: 'new text', x: 300, y: 90 })
    expect(f.puts[1].value.batch).toBe(f.puts[0].value.batch)
    await until('redrawn', () => el.querySelector('.text')?.textContent === 'new text')
    expect(el.style.left).toBe('300px')
  })

  it('resync redraws from the items; bad values are tolerated', async () => {
    const f = fakeK2({ prefix: '' })
    f.seed('obj/a', { id: 'a', text: 'a' })
    mount(f)
    await until('ready', () => f.ready() === 1)
    f.data.clear()
    f.data.set('obj/b', { rev: 50, value: { text: 42, x: 'far', color: '<b>red</b>' }, by: 'agent:x' })
    f.data.set('obj/c', { rev: 51, value: 'not an object', by: 'agent:x' })
    f.resync()
    expect(sticky('obj/a')).toBeNull()
    expect(sticky('obj/b')?.querySelector('.text')?.textContent).toBe('')
    expect(sticky('obj/b')?.style.left).toBe('0px')
    expect(sticky('obj/b')?.dataset.color).toBe('yellow')
    expect(sticky('obj/c')).not.toBeNull()
    expect(document.getElementById('count')?.textContent).toBe('2 notes')
  })

  it("author_unknown says to validate or click It's mine; not_granted says Let … use this data", async () => {
    const f = fakeK2({ listError: 'author_unknown' })
    mount(f)
    await until('ready', () => f.ready() === 1)
    const banner = document.getElementById('note') as HTMLElement
    expect(banner.hidden).toBe(false)
    expect(banner.textContent).toContain('k2 zen validate --widget board-lite')
    expect(banner.textContent).toContain('It’s mine')
    document.body.innerHTML = ''
    const g = fakeK2({ listError: 'not_granted' })
    mount(g)
    await until('ready', () => g.ready() === 1)
    expect(document.getElementById('note')?.textContent).toContain('Let … use this data')
  })

  it('without store.put the board is read-only', async () => {
    const f = fakeK2({ caps: ['store.list', 'store.subscribe'] })
    f.seed('obj/a', { id: 'a', text: 'a' })
    mount(f)
    await until('ready', () => f.ready() === 1)
    expect((document.getElementById('add') as HTMLButtonElement).disabled).toBe(true)
    sticky('obj/a')?.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }))
    expect(sticky('obj/a')?.querySelector('textarea')).toBeNull()
  })
})
