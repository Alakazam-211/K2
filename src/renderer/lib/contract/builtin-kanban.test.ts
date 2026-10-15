// The built-in Kanban widget, k2:kanban@1 (the Kanban Garden, 0.45.7), run
// byte for byte in a fresh JSDOM per test (so no listener from one board
// reaches the next) against a fake `k2` with the Garden data SDK shape
// (contract-garden-data-v1 §7.2): store.list/get/put/delete/batch/tx/
// subscribe, save.get/set, viewer(), agents/thread for the Thread panel.
// K2's real marked + DOMPurify (the vendored stdlib files the frame
// inlines) render the notes. What it pins:
//   - the starter board (To do / Doing / Done + three tiny cards) is
//     written once, with `meta`, in one batch; a board the person emptied
//     is never seeded again, and one an agent started gets `meta` only;
//   - cards: add, edit (only the changed fields), delete; markdown notes
//     render safely;
//   - columns: add, rename, reorder, delete (an empty one, or one with
//     cards: move them, or delete them with it);
//   - dragging a card writes its new column and order;
//   - an agent's write shows live with its handle; a conflict re-reads and
//     re-applies only this gesture's fields;
//   - export is markdown that import reads back;
//   - errors say what to do; the Thread panel is optional and picks nobody.
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

// jsdom (vitest's own DOM) has no type package here: the little this file
// uses, typed by hand.
const { JSDOM } = createRequire(import.meta.url)('jsdom') as {
  JSDOM: new (html: string, opts: { runScripts: 'outside-only'; pretendToBeVisual: boolean }) => { window: unknown }
}

const DIR = resolve(__dirname, '../../../../crates/k2-core/src/zen/builtin_widgets/kanban-1')
const LIB = resolve(__dirname, '../../zen-lib')
const read = (p: string): string => readFileSync(resolve(DIR, p), 'utf8')
const HTML = read('index.html')
const SCRIPTS = ['md.js', 'kanban.js', 'picker.js', 'dock.js'].map(read)
const MARKED = readFileSync(resolve(LIB, 'marked@18.0.14/marked.umd.js'), 'utf8')
const PURIFY = readFileSync(resolve(LIB, 'dompurify@3.4.16/purify.min.js'), 'utf8')

type Json = unknown
interface Stored {
  rev: number
  value: Json
  by: string
}
interface Op {
  op: 'put' | 'delete'
  key: string
  value?: Record<string, unknown>
  ifRev?: number
}
type Delivery = Array<{ key: string; rev: number; value: Json | null; by: string }> | { resync: true; items: unknown[] }

class K2Error extends Error {
  code: string
  constructor(code: string) {
    super(code)
    this.code = code
  }
}

const ALL_CAPS = [
  'store.list',
  'store.get',
  'store.put',
  'store.delete',
  'store.batch',
  'store.changes',
  'save.get',
  'save.set',
  'agents.subscribe',
  'thread.subscribe',
  'thread.post',
  'canvas.setHitRegions',
]

function fakeK2(opts: { caps?: string[]; garden?: string; prefix?: string; listError?: string; data?: Map<string, Stored>; saves?: Map<string, Json> } = {}) {
  const data = opts.data ?? new Map<string, Stored>()
  let head = Math.max(0, ...[...data.values()].map((s) => s.rev))
  const subs: Array<{ prefix: string; cb: (d: Delivery) => void }> = []
  const puts: Array<{ key: string; value: Record<string, unknown>; ifRev: number | undefined }> = []
  const deletes: Array<{ key: string; ifRev: number | undefined }> = []
  const batches: Op[][] = []
  const saves = opts.saves ?? new Map<string, Json>()
  const saveSets: Array<[string, Json]> = []
  const agentSubs: Array<(rows: unknown[]) => void> = []
  const posts: Array<[string, string]> = []
  const hitRegions: unknown[] = []
  let ready = 0
  const push = (changes: Array<{ key: string; rev: number; value: Json | null; by: string }>): void => {
    for (const s of subs) {
      const mine = changes.filter((c) => c.key.startsWith(s.prefix))
      if (mine.length) s.cb(mine)
    }
  }
  const check = (key: string, ifRev: number | undefined): void => {
    if (ifRev !== undefined && (data.get(key)?.rev ?? 0) !== ifRev) throw new K2Error('conflict')
  }
  const k2 = {
    connected: Promise.resolve(),
    config: opts.prefix === undefined ? {} : { prefix: opts.prefix },
    widget: { id: 'board', name: '', garden: opts.garden ?? 'g-test0001' },
    motion: { reduced: false },
    can: (verb: string) => (opts.caps ?? ALL_CAPS).includes(verb),
    ready: () => {
      ready += 1
    },
    viewer: async () => ({ kind: 'owner', name: 'Rosson' }),
    canvas: {
      setHitRegions: async (rects: unknown) => {
        hitRegions.push(rects)
        return null
      },
    },
    save: {
      get: async (key: string) => (saves.has(key) ? saves.get(key) : null),
      set: async (key: string, value: Json) => {
        saves.set(key, value)
        saveSets.push([key, value])
        return null
      },
    },
    agents: {
      subscribe: (cb: (rows: unknown[]) => void) => {
        agentSubs.push(cb)
        return () => undefined
      },
    },
    thread: {
      subscribe: () => () => undefined,
      post: async (to: string, text: string) => {
        posts.push([to, text])
        return { seq: 1 }
      },
      answer: async () => null,
    },
    store: {
      list: async (o: { prefix: string; after?: string; limit?: number }) => {
        if (opts.listError) throw new K2Error(opts.listError)
        const keys = [...data.keys()].filter((k) => k.startsWith(o.prefix) && (!o.after || k > o.after)).sort()
        const page = keys.slice(0, 3) // small pages: the widget must follow `next`
        return {
          items: page.map((k) => ({ key: k, rev: data.get(k)!.rev, value: data.get(k)!.value, updatedAt: 1, by: data.get(k)!.by })),
          next: keys.length > 3 ? page[page.length - 1] : null,
        }
      },
      get: async (key: string) => {
        const s = data.get(key)
        return s ? { key, rev: s.rev, value: s.value, updatedAt: 1, by: s.by } : null
      },
      put: async (key: string, value: Record<string, unknown>, o: { ifRev?: number } = {}) => {
        puts.push({ key, value, ifRev: o.ifRev })
        check(key, o.ifRev)
        head += 1
        data.set(key, { rev: head, value, by: 'Rosson via k2:kanban' })
        push([{ key, rev: head, value, by: 'Rosson via k2:kanban' }])
        return { rev: head }
      },
      delete: async (key: string, o: { ifRev?: number } = {}) => {
        deletes.push({ key, ifRev: o.ifRev })
        check(key, o.ifRev)
        if (!data.has(key)) return { rev: 0, deleted: false }
        head += 1
        data.delete(key)
        push([{ key, rev: head, value: null, by: 'Rosson via k2:kanban' }])
        return { rev: head, deleted: true }
      },
      batch: async (ops: Op[]) => {
        batches.push(ops)
        if (ops.length > 64) throw new K2Error('too_large')
        for (const op of ops) check(op.key, op.ifRev)
        head += 1
        const changes = ops.map((op) => {
          if (op.op === 'delete') data.delete(op.key)
          else data.set(op.key, { rev: head, value: op.value, by: 'Rosson via k2:kanban' })
          return { key: op.key, rev: head, value: op.op === 'delete' ? null : (op.value as Json), by: 'Rosson via k2:kanban' }
        })
        push(changes)
        return { ops: ops.length, rev: head }
      },
      subscribe: (prefix: string, cb: (d: Delivery) => void) => {
        subs.push({ prefix, cb })
        return () => undefined
      },
    },
  }
  return {
    k2,
    data,
    puts,
    deletes,
    batches,
    subs,
    saves,
    saveSets,
    agentSubs,
    posts,
    hitRegions,
    ready: () => ready,
    /** An agent (or another window) writes; the widget hears it live. */
    agentWrite: (key: string, value: Json | null, by = 'agent:board-keeper') => {
      head += 1
      if (value === null) data.delete(key)
      else data.set(key, { rev: head, value, by })
      push([{ key, rev: head, value, by }])
      return head
    },
    /** A write the widget hasn't heard about (no push). */
    sneak: (key: string, value: Json, by = 'agent:board-keeper') => {
      head += 1
      data.set(key, { rev: head, value, by })
      return head
    },
    resync: () => {
      const items = [...data.entries()].map(([key, s]) => ({ key, rev: s.rev, value: s.value, by: s.by }))
      for (const s of subs) s.cb({ resync: true, items: items.filter((i) => i.key.startsWith(s.prefix)) })
    },
  }
}

type Fake = ReturnType<typeof fakeK2>

function mount(f: Fake) {
  const dom = new JSDOM(HTML.replace(/<script[^>]*><\/script>/g, ''), { runScripts: 'outside-only', pretendToBeVisual: true })
  const win = dom.window as unknown as Window & typeof globalThis & { eval: (s: string) => unknown; k2: unknown }
  win.k2 = f.k2
  win.eval(MARKED)
  win.eval(PURIFY)
  for (const s of SCRIPTS) win.eval(s)
  const doc = win.document
  const $ = <T extends HTMLElement = HTMLElement>(sel: string): T => {
    const n = doc.querySelector<T>(sel)
    if (!n) throw new Error(`no ${sel}`)
    return n
  }
  const columns = (): string[] => [...doc.querySelectorAll('.col:not(.add-col) .col-name')].map((n) => n.textContent ?? '')
  const cardsIn = (colId: string): string[] =>
    [...doc.querySelectorAll(`.col[data-col="${colId}"] .card .card-title`)].map((n) => n.textContent ?? '')
  const card = (key: string): HTMLElement | null => doc.querySelector<HTMLElement>(`.card[data-key="${key}"]`)
  const type = (el: HTMLInputElement | HTMLTextAreaElement, text: string): void => {
    el.value = text
    el.dispatchEvent(new win.Event('input', { bubbles: true }))
  }
  const key = (el: Element, k: string): void => {
    el.dispatchEvent(new win.KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true }))
  }
  const pointer = (el: Element, kind: string, x: number, y: number): void => {
    const e = new win.MouseEvent(kind, { bubbles: true, cancelable: true, clientX: x, clientY: y, button: 0 })
    Object.defineProperty(e, 'pointerId', { value: 1 })
    Object.defineProperty(e, 'pointerType', { value: 'mouse' })
    el.dispatchEvent(e)
  }
  return { dom, win, doc, $, columns, cardsIn, card, type, key, pointer }
}

const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 0))
async function until(what: string, ok: () => boolean): Promise<void> {
  for (let i = 0; i < 300; i++) {
    if (ok()) return
    await tick()
  }
  throw new Error(`timed out waiting for: ${what}`)
}

const P = 'board/g-test0001/'

/** A board as K2 would hold it: meta + the given columns and cards. */
function board(cols: Array<[string, string, number]>, cards: Array<[string, string, string, number, string?]>): Map<string, Stored> {
  const m = new Map<string, Stored>()
  let rev = 0
  m.set(`${P}meta`, { rev: ++rev, value: { v: 1, seeded: true }, by: 'you' })
  for (const [id, name, order] of cols) m.set(`${P}col/${id}`, { rev: ++rev, value: { id, name, order }, by: 'Rosson via k2:kanban' })
  for (const [id, title, col, order, note] of cards) {
    m.set(`${P}card/${id}`, { rev: ++rev, value: { id, title, note: note ?? '', col, order }, by: 'Rosson via k2:kanban' })
  }
  return m
}

const THREE = (): Map<string, Stored> =>
  board(
    [
      ['todo', 'To do', 1024],
      ['doing', 'Doing', 2048],
      ['done', 'Done', 3072],
    ],
    [
      ['a', 'Alpha', 'todo', 1024],
      ['b', 'Beta', 'todo', 2048],
      ['c', 'Gamma', 'doing', 1024],
    ],
  )

async function started(f: Fake) {
  const m = mount(f)
  await until('ready', () => f.ready() === 1)
  return m
}

describe('k2:kanban@1', () => {
  it('is sealed-frame clean: no innerHTML, inline handlers, network or browser storage', () => {
    for (const [i, src] of SCRIPTS.entries()) {
      const code = src
        .split('\n')
        .filter((l) => !l.trim().startsWith('//'))
        .join('\n')
      expect(code, ['md.js', 'kanban.js', 'picker.js', 'dock.js'][i]).not.toMatch(/innerHTML|outerHTML|insertAdjacentHTML|fetch\(|XMLHttpRequest|localStorage|sessionStorage|indexedDB|eval\(|new Function/)
    }
    expect(HTML).not.toMatch(/ on[a-z]+=/)
    const m = JSON.parse(read('manifest.json'))
    expect(m.caps).toEqual(['agents:read', 'thread:read', 'thread:post', 'store:read', 'store:write'])
    expect(m.requires.libs).toEqual(['marked@18', 'dompurify@3'])
  })

  it('seeds the starter board once, in one batch with meta, under this Garden’s prefix', async () => {
    const f = fakeK2()
    const { columns, cardsIn, doc } = await started(f)
    expect(f.batches).toHaveLength(1)
    const keys = f.batches[0].map((o) => o.key)
    expect(keys).toEqual([
      `${P}meta`,
      `${P}col/todo`,
      `${P}col/doing`,
      `${P}col/done`,
      `${P}card/start-drag`,
      `${P}card/start-notes`,
      `${P}card/start-agents`,
    ])
    expect(f.batches[0].every((o) => o.op === 'put' && o.ifRev === 0)).toBe(true)
    expect(columns()).toEqual(['To do', 'Doing', 'Done'])
    expect(cardsIn('todo')).toEqual(['Drag me to Doing'])
    expect(cardsIn('doing')).toEqual(['Notes take markdown'])
    expect(cardsIn('done')).toEqual(['Your agents can help'])
    // The starter notes render as markdown (K2's marked + DOMPurify).
    expect(doc.querySelector('.col[data-col="doing"] .card-note strong')?.textContent).toBe('Bold')
    // Nothing from K2's own roadmap ships.
    expect(doc.body.textContent).not.toMatch(/0\.4[56]\.|Parked|Someone else|ranking/)
    expect(f.subs.map((s) => s.prefix)).toEqual([P])

    // The person empties the board; the next load seeds nothing.
    const again = fakeK2({ data: new Map([[`${P}meta`, f.data.get(`${P}meta`)!]]) })
    const m2 = await started(again)
    expect(again.batches).toEqual([])
    expect(again.puts).toEqual([])
    expect(m2.columns()).toEqual([])
    expect(m2.doc.querySelector('#addColumn')).not.toBeNull()
    expect(m2.doc.querySelector('.board-empty')?.textContent).toBe('No columns yet.')
  })

  it('a board an agent started first gets meta only; config.prefix names another board', async () => {
    const data = new Map<string, Stored>([[`${P}card/x`, { rev: 1, value: { title: 'From an agent', col: 'nowhere' }, by: 'agent:board-keeper' }]])
    const f = fakeK2({ data })
    const { columns } = await started(f)
    expect(f.batches.map((b) => b.map((o) => o.key))).toEqual([[`${P}meta`]])
    expect(columns()).toEqual([])

    const g = fakeK2({ prefix: 'team/board/' })
    await started(g)
    expect(g.batches[0][0].key).toBe('team/board/meta')
    expect(g.subs.map((s) => s.prefix)).toEqual(['team/board/'])
  })

  it('adds a card at the end of its column and keeps the box open for the next', async () => {
    const f = fakeK2({ data: THREE() })
    const m = await started(f)
    m.$('.col[data-col="todo"] .add-card').click()
    const box = m.$<HTMLTextAreaElement>('.col[data-col="todo"] .add-card-input')
    expect(m.doc.activeElement).toBe(box)
    m.type(box, 'Delta')
    m.key(box, 'Enter')
    await until('a put', () => f.puts.length === 1)
    const put = f.puts[0]
    expect(put.key.startsWith(`${P}card/`)).toBe(true)
    expect(put.ifRev).toBe(0)
    expect(put.value).toMatchObject({ title: 'Delta', note: '', col: 'todo', order: 3072 })
    expect(put.value.id).toBe(put.key.slice(`${P}card/`.length))
    await until('drawn', () => m.cardsIn('todo').join() === 'Alpha,Beta,Delta')
    expect(m.doc.querySelector('.col[data-col="todo"] .add-card-input')).not.toBeNull()
    expect(m.doc.querySelector(`.card[data-key="${put.key}"] .card-by`)?.textContent).toBe('Rosson')
  })

  it('edits only the changed fields, renders the note as safe markdown, and deletes', async () => {
    const f = fakeK2({ data: THREE() })
    const m = await started(f)
    m.card(`${P}card/a`)!.click()
    expect(m.$('#editor').hidden).toBe(false)
    expect(m.$<HTMLInputElement>('#edTitle').value).toBe('Alpha')
    m.$<HTMLTextAreaElement>('#edNote').value = '**big** <img src=x onerror="alert(1)"><script>alert(2)</script> [k2](https://k2.dev)'
    m.$('#edSave').click()
    await until('a put', () => f.puts.length === 1)
    expect(Object.keys(f.puts[0].value).sort()).toEqual(['batch', 'col', 'id', 'note', 'order', 'title'])
    expect(f.puts[0].value.title).toBe('Alpha')
    expect(f.puts[0].ifRev).toBe(THREE().get(`${P}card/a`)!.rev)
    await until('note drawn', () => m.card(`${P}card/a`)?.querySelector('.card-note strong') != null)
    const note = m.card(`${P}card/a`)!.querySelector('.card-note')!
    expect(note.querySelector('strong')?.textContent).toBe('big')
    expect(note.querySelector('img, script, a')).toBeNull()
    expect(note.textContent).toContain('<img src=x onerror="alert(1)">')
    expect(note.querySelector('.md-url')?.textContent).toBe('https://k2.dev')
    expect(m.$('#editor').hidden).toBe(true)

    // Change the column from the editor: it goes to the end of Doing.
    m.card(`${P}card/b`)!.click()
    m.$<HTMLSelectElement>('#edCol').value = 'doing'
    m.$('#edSave').click()
    await until('moved', () => m.cardsIn('doing').join() === 'Gamma,Beta')
    expect(f.puts[1].value).toMatchObject({ col: 'doing', order: 2048, title: 'Beta' })

    // Delete asks first, then deletes with the rev it read.
    m.card(`${P}card/c`)!.click()
    m.$('#edDelete').click()
    expect(m.$('#confirm').hidden).toBe(false)
    expect(m.$('#confirmTitle').textContent).toBe('Delete “Gamma”?')
    m.$('#confirmActions [data-action="delete"]').click()
    await until('deleted', () => f.deletes.length === 1)
    expect(f.deletes[0]).toEqual({ key: `${P}card/c`, ifRev: THREE().get(`${P}card/c`)!.rev })
    await until('gone', () => m.card(`${P}card/c`) === null)
  })

  it('columns: add, rename, move, and delete an empty one', async () => {
    const f = fakeK2({ data: THREE() })
    const m = await started(f)
    m.$('#addColumn').click()
    const name = m.$<HTMLInputElement>('.add-col-input')
    m.type(name, 'Blocked')
    m.key(name, 'Enter')
    await until('column added', () => m.columns().join() === 'To do,Doing,Done,Blocked')
    expect(f.puts[0].value).toMatchObject({ name: 'Blocked', order: 4096 })
    expect(f.puts[0].ifRev).toBe(0)
    const blocked = f.puts[0].key

    // Rename by double-click.
    m.$(`.col[data-key="${blocked}"] .col-name`).dispatchEvent(new m.win.MouseEvent('dblclick', { bubbles: true }))
    const rename = m.$<HTMLInputElement>('.col-rename')
    m.type(rename, 'Waiting')
    m.key(rename, 'Enter')
    await until('renamed', () => m.columns().join() === 'To do,Doing,Done,Waiting')
    expect(f.puts[1]).toMatchObject({ key: blocked, value: { name: 'Waiting', order: 4096 } })

    // Move left from the column menu: between Doing and Done.
    m.$(`.col[data-key="${blocked}"] .col-menu-btn`).click()
    const left = [...m.doc.querySelectorAll<HTMLButtonElement>('.col-menu-item')].find((b) => b.textContent === 'Move left')!
    left.click()
    await until('moved left', () => m.columns().join() === 'To do,Doing,Waiting,Done')
    expect(f.puts[2].value.order).toBe(2560)

    // Delete the (empty) column.
    m.$(`.col[data-key="${blocked}"] .col-menu-btn`).click()
    ;[...m.doc.querySelectorAll<HTMLButtonElement>('.col-menu-item')].find((b) => b.textContent === 'Delete column…')!.click()
    await until('asks', () => !m.$('#confirm').hidden)
    expect(m.$('#confirmText').textContent).toBe('The column is empty.')
    m.$('#confirmActions [data-action="delete"]').click()
    await until('column gone', () => m.columns().join() === 'To do,Doing,Done')
    expect(f.deletes.map((d) => d.key)).toEqual([blocked])
  })

  it('deleting a column with cards asks: move them elsewhere, or delete them with it', async () => {
    const f = fakeK2({ data: THREE() })
    const m = await started(f)
    m.$('.col[data-col="todo"] .col-menu-btn').click()
    ;[...m.doc.querySelectorAll<HTMLButtonElement>('.col-menu-item')].find((b) => b.textContent === 'Delete column…')!.click()
    await until('asks', () => !m.$('#confirm').hidden)
    expect(m.$('#confirmText').textContent).toContain('It has 2 cards.')
    const pick = m.$<HTMLSelectElement>('#confirmMoveTo')
    expect([...pick.options].map((o) => o.value)).toEqual(['doing', 'done'])
    pick.value = 'done'
    m.$('#confirmActions [data-action="move"]').click()
    await until('one batch', () => f.batches.length === 1)
    const ops = f.batches[0]
    expect(ops.map((o) => [o.op, o.key])).toEqual([
      ['put', `${P}card/a`],
      ['put', `${P}card/b`],
      ['delete', `${P}col/todo`],
    ])
    expect(ops[0].value).toMatchObject({ title: 'Alpha', col: 'done', order: 1024 })
    expect(ops[1].value).toMatchObject({ title: 'Beta', col: 'done', order: 2048 })
    await until('moved', () => m.columns().join() === 'Doing,Done' && m.cardsIn('done').join() === 'Alpha,Beta')

    // Delete Doing and its card with it.
    m.$('.col[data-col="doing"] .col-menu-btn').click()
    ;[...m.doc.querySelectorAll<HTMLButtonElement>('.col-menu-item')].find((b) => b.textContent === 'Delete column…')!.click()
    await until('asks again', () => !m.$('#confirm').hidden)
    m.$('#confirmActions [data-action="delete"]').click()
    await until('second batch', () => f.batches.length === 2)
    expect(f.batches[1].map((o) => [o.op, o.key])).toEqual([
      ['delete', `${P}card/c`],
      ['delete', `${P}col/doing`],
    ])
    await until('only Done', () => m.columns().join() === 'Done')
    expect(f.data.has(`${P}card/c`)).toBe(false)
  })

  it('dragging a card to another column, and within one, writes its column and order', async () => {
    const f = fakeK2({ data: THREE() })
    const m = await started(f)
    const doingBody = m.$('.col[data-col="doing"] .col-body')
    // Layout: Gamma sits at y 100–140 in Doing.
    const gamma = m.card(`${P}card/c`)!
    gamma.getBoundingClientRect = () => ({ top: 100, height: 40, left: 300, width: 200, bottom: 140, right: 500, x: 300, y: 100, toJSON: () => ({}) })
    ;(m.doc as Document & { elementFromPoint: (x: number, y: number) => Element | null }).elementFromPoint = () => doingBody
    const alpha = m.card(`${P}card/a`)!
    m.pointer(alpha, 'pointerdown', 10, 10)
    m.pointer(m.win.document.body, 'pointermove', 30, 60)
    expect(m.doc.querySelector('.drag-ghost')).not.toBeNull()
    expect(alpha.classList.contains('dragging')).toBe(true)
    // Above Gamma's middle: the placeholder goes before it.
    m.pointer(m.win.document.body, 'pointermove', 320, 90)
    expect(doingBody.firstElementChild?.id).toBe('ph')
    m.pointer(m.win.document.body, 'pointerup', 320, 90)
    await until('a put', () => f.puts.length === 1)
    expect(f.puts[0]).toMatchObject({ key: `${P}card/a`, value: { col: 'doing', order: 0, title: 'Alpha' } })
    await until('drawn', () => m.cardsIn('doing').join() === 'Alpha,Gamma')
    expect(m.doc.querySelector('.drag-ghost')).toBeNull()

    // Esc puts a lifted card back: nothing is written.
    const beta = m.card(`${P}card/b`)!
    m.pointer(beta, 'pointerdown', 10, 10)
    m.pointer(m.win.document.body, 'pointermove', 40, 40)
    m.key(m.doc.body, 'Escape')
    m.pointer(m.win.document.body, 'pointerup', 40, 40)
    await tick()
    expect(f.puts).toHaveLength(1)

    // Keyboard: Down moves Alpha under Gamma (between Gamma and the end).
    await until('alpha redrawn', () => m.card(`${P}card/a`) !== null)
    m.key(m.card(`${P}card/a`)!, 'ArrowDown')
    await until('second put', () => f.puts.length === 2)
    expect(f.puts[1].value).toMatchObject({ col: 'doing', order: 2048 })
    await until('reordered', () => m.cardsIn('doing').join() === 'Gamma,Alpha')
  })

  it('renumbers a column in one batch when an agent left out the order', async () => {
    const data = board([['todo', 'To do', 1024]], [['a', 'Alpha', 'todo', 1024]])
    data.set(`${P}card/y`, { rev: 50, value: { title: 'Y', col: 'todo' }, by: 'agent:board-keeper' })
    data.set(`${P}card/z`, { rev: 51, value: { title: 'Z', col: 'todo' }, by: 'agent:board-keeper' })
    const aRev = data.get(`${P}card/a`)!.rev
    const f = fakeK2({ data })
    const m = await started(f)
    expect(m.cardsIn('todo')).toEqual(['Alpha', 'Y', 'Z'])
    m.key(m.card(`${P}card/z`)!, 'ArrowUp')
    await until('a batch', () => f.batches.length === 1)
    expect(f.batches[0].map((o) => [o.key, o.value?.order, o.ifRev])).toEqual([
      [`${P}card/a`, 1024, aRev],
      [`${P}card/z`, 2048, 51],
      [`${P}card/y`, 3072, 50],
    ])
    await until('drawn', () => m.cardsIn('todo').join() === 'Alpha,Z,Y')
  })

  it("an agent's write shows live with its handle; deletes and a resync redraw; bad values don't break it", async () => {
    const f = fakeK2({ data: THREE() })
    const m = await started(f)
    f.agentWrite(`${P}card/k`, { title: 'From the keeper', note: 'hi', col: 'done', order: 1 })
    expect(m.cardsIn('done')).toEqual(['From the keeper'])
    const k = m.card(`${P}card/k`)!
    expect(k.querySelector('.card-by')?.textContent).toBe('board-keeper')
    expect(k.classList.contains('by-agent')).toBe(true)
    f.agentWrite(`${P}col/later`, { name: 'Later', order: 5000 })
    expect(m.columns()).toEqual(['To do', 'Doing', 'Done', 'Later'])
    f.agentWrite(`${P}card/k`, null)
    expect(m.card(`${P}card/k`)).toBeNull()
    // Bad shapes: no title, a non-string col, a string order, a non-object.
    f.data.set(`${P}card/bad1`, { rev: 90, value: { title: 7, col: 3, order: 'x' }, by: 'agent:x' })
    f.data.set(`${P}card/bad2`, { rev: 91, value: 'not an object', by: 'agent:x' })
    f.resync()
    expect(m.cardsIn('todo')).toEqual(['Alpha', 'Beta', 'Untitled', 'Untitled'])
    expect(m.columns()).toEqual(['To do', 'Doing', 'Done', 'Later'])
  })

  it('field-level: an edit that hits a conflict re-reads and re-applies only its own field', async () => {
    const f = fakeK2({ data: THREE() })
    const m = await started(f)
    const seen = f.data.get(`${P}card/a`)!.rev
    // The agent moves the card while the widget still holds the old rev.
    const moved = f.sneak(`${P}card/a`, { id: 'a', title: 'Alpha', note: 'agent note', col: 'done', order: 7 })
    m.card(`${P}card/a`)!.click()
    m.$<HTMLInputElement>('#edTitle').value = 'Alpha, renamed'
    m.$('#edSave').click()
    await until('two puts', () => f.puts.length === 2)
    expect(f.puts[0].ifRev).toBe(seen)
    expect(f.puts[1].ifRev).toBe(moved)
    expect(f.puts[1].value).toMatchObject({ title: 'Alpha, renamed', note: 'agent note', col: 'done', order: 7 })
    expect(f.puts[1].value.batch).toBe(f.puts[0].value.batch)
    await until('redrawn', () => m.cardsIn('done').join() === 'Alpha, renamed')
  })

  it('exports markdown that import reads back, adding cards and new columns', async () => {
    const data = THREE()
    data.set(`${P}card/a`, { rev: 40, value: { id: 'a', title: 'Alpha *one*', note: 'line 1\n\n- item', col: 'todo', order: 1024 }, by: 'you' })
    const f = fakeK2({ data })
    const m = await started(f)
    const md = (m.win as unknown as { KanbanBoard: { exportMarkdown: () => string } }).KanbanBoard.exportMarkdown()
    expect(md).toBe(
      [
        '# Kanban',
        '',
        '## To do',
        '- **Alpha \\*one\\***',
        '  line 1',
        '',
        '  - item',
        '- **Beta**',
        '',
        '## Doing',
        '- **Gamma**',
        '',
        '## Done',
        '_(no cards)_',
        '',
      ].join('\n'),
    )
    // The Export button: no clipboard in this frame, so the text shows.
    m.$('#btnExport').click()
    await until('export panel', () => !m.$('#textPanel').hidden)
    expect(m.$<HTMLTextAreaElement>('#textBody').value).toBe(md)
    m.$('#textClose').click()

    m.$('#btnImport').click()
    m.$<HTMLTextAreaElement>('#textBody').value = md + '\n## Someday\n1. **Delta** — later\n'
    m.$('#textGo').click()
    await until('one batch', () => f.batches.length === 1)
    const ops = f.batches[0]
    const added = ops.filter((o) => o.key.startsWith(`${P}card/`)).map((o) => [o.value?.title, o.value?.note])
    expect(added).toEqual([
      ['Alpha *one*', 'line 1\n\n- item'],
      ['Beta', ''],
      ['Gamma', ''],
      ['Delta', 'later'],
    ])
    expect(ops.every((o) => o.op === 'put' && o.ifRev === 0)).toBe(true)
    await until('drawn', () => m.columns().join() === 'To do,Doing,Done,Someday')
    expect(m.cardsIn('todo')).toEqual(['Alpha *one*', 'Beta', 'Alpha *one*', 'Beta'])
    expect(m.cardsIn('done')).toEqual([])
  })

  it('For agents names the widget, this board’s prefix and the grant to give', async () => {
    const f = fakeK2({ data: THREE() })
    const m = await started(f)
    m.$('#btnAgents').click()
    const text = m.$<HTMLTextAreaElement>('#textBody').value
    expect(text).toContain(`k2 zen data grant k2:kanban --agent <handle> --level write --prefix ${P}`)
    expect(text).toContain(`k2 store put --widget k2:kanban ${P}card/<new-id>`)
    expect(text).toContain('todo (To do), doing (Doing), done (Done)')
  })

  it('says what to do when Garden data is missing or refused, and is read-only without store:write', async () => {
    const old = fakeK2({ caps: ['agents.subscribe', 'thread.subscribe'] })
    const m = await started(old)
    expect(m.$('#banner').textContent).toContain('needs K2 0.45.6 or later')
    expect(old.batches).toEqual([])

    const web = fakeK2({ listError: 'verb_local' })
    const w = await started(web)
    expect(w.$('#banner').textContent).toContain('computer that runs K2')

    const ro = fakeK2({ data: THREE(), caps: ['store.list', 'store.changes'] })
    const r = await started(ro)
    expect(r.$('#banner').textContent).toBe('This board is read-only here.')
    expect(r.doc.querySelector('.add-card, #addColumn, .col-menu-btn')).toBeNull()
    r.card(`${P}card/a`)!.click()
    expect(r.$<HTMLInputElement>('#edTitle').disabled).toBe(true)
    expect(r.$('#edSave').hidden).toBe(true)
    expect(ro.batches).toEqual([])
  })

  it('the Thread panel is optional: closed, nobody picked, agents read only once opened; saved per Garden', async () => {
    const f = fakeK2({ data: THREE() })
    const m = await started(f)
    await until('Thread button', () => !m.$('#btnThread').hidden)
    expect(m.$('#threadDock').hidden).toBe(true)
    expect(f.agentSubs).toHaveLength(0)
    m.$('#btnThread').click()
    expect(m.$('#threadDock').hidden).toBe(false)
    expect(f.agentSubs).toHaveLength(1)
    f.agentSubs[0]([{ address: 'cortana::local', label: 'Cortana', state: 'idle' }])
    expect(m.$('.ap-trigger .ap-name').textContent).toBe('Pick an agent')
    expect(m.$('#dockBody').textContent).toBe('Pick an agent above to talk to it here.')
    expect((m.$('#btnSend') as HTMLButtonElement).disabled).toBe(true)
    await until('saved', () => f.saveSets.length >= 1)
    expect(f.saveSets.at(-1)).toEqual(['dock/g-test0001', { open: true, agent: null }])

    // Pick Cortana; Send board posts the markdown export to her.
    m.$('.ap-trigger').click()
    m.$('.ap-row[data-address="cortana::local"]').click()
    expect((m.$('#btnSend') as HTMLButtonElement).disabled).toBe(false)
    m.$('#btnSend').click()
    await until('posted', () => f.posts.length === 1)
    expect(f.posts[0][0]).toBe('cortana::local')
    expect(f.posts[0][1]).toContain('## To do')
    expect(f.saveSets.at(-1)).toEqual(['dock/g-test0001', { open: true, agent: 'cortana::local' }])

    // Without the Thread caps there's no Thread button at all.
    const g = fakeK2({ data: THREE(), caps: ALL_CAPS.filter((c) => !c.startsWith('thread.') && !c.startsWith('agents.')) })
    const n = await started(g)
    await tick()
    expect(n.$('#btnThread').hidden).toBe(true)
  })

  it('makes its toolbar buttons holes in K2’s top band', async () => {
    const f = fakeK2({ data: THREE() })
    await started(f)
    expect(f.hitRegions.length).toBeGreaterThan(0)
    expect(Array.isArray(f.hitRegions[0])).toBe(true)
  })
})
