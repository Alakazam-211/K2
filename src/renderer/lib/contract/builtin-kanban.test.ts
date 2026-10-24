// The built-in Kanban widget, k2:kanban@1 (the Kanban Garden, 0.45.7) and
// k2:kanban@2 (0.46.1: the Thread panel in K2's Thread look), each run
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
// Every version passes all of that. @2 and @3 also pin the Thread panel's message
// flow: bubbles (yours right, the agent's left) with "author · time" and
// markdown, Enter sends and Shift+Enter doesn't, the round Send, posts reach
// the agent and its replies render, the working ear's state, counters and
// clock, and K2 Companion's look by default (look = "garden" opts out).
// @3 and @4 keep a scrolled-up reader in place; @4 (Thread parity) also
// draws files and pictures, links workspace paths, takes picked files in
// the message box, marks read, catches up and pages back.
import { existsSync, readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'
import { beforeAll, describe, expect, it } from 'vitest'

// jsdom (vitest's own DOM) has no type package here: the little this file
// uses, typed by hand.
const { JSDOM } = createRequire(import.meta.url)('jsdom') as {
  JSDOM: new (html: string, opts: { runScripts: 'outside-only'; pretendToBeVisual: boolean }) => { window: unknown }
}

const WIDGETS = resolve(__dirname, '../../../../crates/k2-core/src/zen/builtin_widgets')
const LIB = resolve(__dirname, '../../zen-lib')
// The version under test (each describe block sets it before its tests run).
let DIR = resolve(WIDGETS, 'kanban-1')
// @3 ships only the files it changed (dock.js, kanban.css); the rest are
// @2's, as BUILTIN_WIDGETS lists them.
const read = (p: string): string =>
  readFileSync(existsSync(resolve(DIR, p)) ? resolve(DIR, p) : resolve(WIDGETS, 'kanban-2', p), 'utf8')
let HTML = read('index.html')
// @1–@4 draw their own Thread panel (picker.js, dock.js); @5 (0.46.4) is
// the board beside K2's own Conversation (thread.js only folds its column
// and sends the board).
type KanbanDir = 'kanban-1' | 'kanban-2' | 'kanban-3' | 'kanban-4' | 'kanban-5'
const scriptNames = (dir: KanbanDir): string[] =>
  dir === 'kanban-5' ? ['md.js', 'kanban.js', 'thread.js'] : ['md.js', 'kanban.js', 'picker.js', 'dock.js']
let SCRIPT_NAMES = scriptNames('kanban-1')
let SCRIPTS = SCRIPT_NAMES.map(read)
function useVersion(dir: KanbanDir): void {
  DIR = resolve(WIDGETS, dir)
  HTML = read('index.html')
  SCRIPT_NAMES = scriptNames(dir)
  SCRIPTS = SCRIPT_NAMES.map(read)
}
const MARKED = readFileSync(resolve(LIB, 'marked@18.0.14/marked.umd.js'), 'utf8')
const PURIFY = readFileSync(resolve(LIB, 'dompurify@3.4.17/purify.min.js'), 'utf8')

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

interface LayoutColumn {
  index: number
  label: string
  collapsible: boolean
  collapsed: boolean
}

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
  // Thread parity (catalog v16): @4 uses them; @1–@3 never ask.
  'thread.read',
  'thread.markRead',
  'thread.attachment',
  'thread.open',
  'thread.links',
  'thread.pickFiles',
  // @5 (0.46.4): its Thread column and the Conversation's shared pick;
  // @1–@4 never ask.
  'layout.get',
  'layout.toggle',
  'layout.subscribe',
  'conversation.open',
  'save.remove',
]
/** What K2 before Thread parity answers `k2.can` for (no files, links, read). */
const PRE_PARITY_CAPS = ALL_CAPS.filter((v) => !/^thread\.(read|markRead|attachment|open|links|pickFiles)$/.test(v))

function fakeK2(
  opts: {
    caps?: string[]
    garden?: string
    prefix?: string
    config?: Record<string, unknown>
    listError?: string
    postError?: string
    data?: Map<string, Stored>
    saves?: Map<string, Json>
    /** thread.attachment thumb answers by `docId/n` (a missing key refuses). */
    thumbs?: Record<string, { mime: string; base64: string }>
    /** thread.links: which inputs lead into the workspace, input → rel. */
    linkable?: Record<string, string>
    /** thread.pickFiles answers, in turn. */
    picks?: Array<{ files: Array<{ relPath: string; name: string }> }>
    /** thread.read answers for afterSeq / beforeSeq reads. */
    reads?: (opts: Record<string, unknown>) => unknown
    /** @5: the page's columns, as k2.layout.get answers. */
    columns?: LayoutColumn[]
  } = {},
) {
  const data = opts.data ?? new Map<string, Stored>()
  let head = Math.max(0, ...[...data.values()].map((s) => s.rev))
  const subs: Array<{ prefix: string; cb: (d: Delivery) => void }> = []
  const puts: Array<{ key: string; value: Record<string, unknown>; ifRev: number | undefined }> = []
  const deletes: Array<{ key: string; ifRev: number | undefined }> = []
  const batches: Op[][] = []
  const saves = opts.saves ?? new Map<string, Json>()
  const saveSets: Array<[string, Json]> = []
  const agentSubs: Array<(rows: unknown[]) => void> = []
  const posts: Array<[string, string] | [string, string, unknown]> = []
  const attachmentCalls: Array<[string, string, number, unknown]> = []
  const opens: Array<[string, unknown]> = []
  const linkCalls: Array<[string, string[]]> = []
  const pickCalls: string[] = []
  const markReads: Array<[string, unknown]> = []
  const reads: Array<[string, unknown]> = []
  const answers: Array<[string, string, string]> = []
  const threadSubs: Array<{ address: string; cb: (view: unknown) => void }> = []
  let nextSeq = 100
  // @5: the page's columns (k2.layout), the Conversation's shared pick
  // (k2.conversation.open) and the save it carries over.
  let columns: LayoutColumn[] = (opts.columns ?? []).map((c) => ({ ...c }))
  const layoutSubs: Array<(info: { columns: LayoutColumn[] }) => void> = []
  const toggles: number[] = []
  const convOpens: string[] = []
  const saveRemoves: string[] = []
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
    config: opts.config ?? (opts.prefix === undefined ? {} : { prefix: opts.prefix }),
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
      remove: async (key: string) => {
        saves.delete(key)
        saveRemoves.push(key)
        return null
      },
    },
    layout: {
      get: async () => ({ columns: columns.map((c) => ({ ...c })) }),
      toggle: async (index: number) => {
        toggles.push(index)
        const col = columns.find((c) => c.index === index)
        if (!col || !col.collapsible) throw new K2Error('failed')
        columns = columns.map((c) => (c.index === index ? { ...c, collapsed: !c.collapsed } : c))
        const now = columns.find((c) => c.index === index)!
        for (const cb of layoutSubs) cb({ columns: columns.map((c) => ({ ...c })) })
        return { ...now }
      },
      subscribe: (cb: (info: { columns: LayoutColumn[] }) => void) => {
        layoutSubs.push(cb)
        return () => undefined
      },
    },
    conversation: {
      open: async (address: string) => {
        convOpens.push(address)
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
      subscribe: (address: string, cb: (view: unknown) => void) => {
        threadSubs.push({ address, cb })
        return () => undefined
      },
      post: async (...args: [string, string] | [string, string, unknown]) => {
        posts.push(args)
        if (opts.postError) throw new K2Error(opts.postError)
        nextSeq += 1
        return { id: `p${nextSeq}`, seq: nextSeq }
      },
      answer: async (to: string, id: string, answer: string) => {
        answers.push([to, id, answer])
        return null
      },
      attachment: async (to: string, docId: string, n: number, o: unknown) => {
        attachmentCalls.push([to, docId, n, o])
        const t = opts.thumbs?.[`${docId}/${n}`]
        if (!t) throw new K2Error('failed')
        return { kind: 'image', mime: t.mime, base64: t.base64, width: 10, height: 10 }
      },
      open: async (to: string, target: unknown) => {
        opens.push([to, target])
        return { opened: true }
      },
      links: async (to: string, candidates: string[]) => {
        linkCalls.push([to, candidates])
        return {
          links: candidates.map((input) => {
            const rel = opts.linkable?.[input]
            return rel ? { input, linkable: true, kind: 'file', rel } : { input, linkable: false }
          }),
        }
      },
      pickFiles: async (to: string) => {
        pickCalls.push(to)
        return opts.picks?.shift() ?? { files: [] }
      },
      markRead: async (to: string, o: unknown) => {
        markReads.push([to, o])
        return { changed: true, lastReadSeq: 0, lastSeq: 0, unread: 0 }
      },
      read: async (to: string, o: Record<string, unknown>) => {
        reads.push([to, o])
        if (!opts.reads) throw new K2Error('failed')
        return opts.reads(o)
      },
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
    answers,
    threadSubs,
    attachmentCalls,
    opens,
    linkCalls,
    pickCalls,
    markReads,
    reads,
    hitRegions,
    layoutSubs,
    toggles,
    convOpens,
    saveRemoves,
    /** The person folds or unfolds a column with K2's own tab or chevron. */
    fold: (index: number, collapsed: boolean) => {
      columns = columns.map((c) => (c.index === index ? { ...c, collapsed } : c))
      for (const cb of layoutSubs) cb({ columns: columns.map((c) => ({ ...c })) })
    },
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
  const win = dom.window as unknown as Window &
    typeof globalThis & {
      eval: (s: string) => unknown
      k2: unknown
      KanbanDock: { look(): string; address(): string; chosen(): string; isOpen(): boolean }
      KanbanThread: { look(): string; column(): number; shown(): string }
    }
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

describe.each(['kanban-1', 'kanban-2', 'kanban-3', 'kanban-4', 'kanban-5'] as const)('k2:kanban (%s)', (dir) => {
  beforeAll(() => useVersion(dir))

  it('is sealed-frame clean: no innerHTML, inline handlers, network or browser storage', () => {
    for (const [i, src] of SCRIPTS.entries()) {
      const code = src
        .split('\n')
        .filter((l) => !l.trim().startsWith('//'))
        .join('\n')
      expect(code, SCRIPT_NAMES[i]).not.toMatch(/innerHTML|outerHTML|insertAdjacentHTML|fetch\(|XMLHttpRequest|localStorage|sessionStorage|indexedDB|eval\(|new Function/)
    }
    expect(HTML).not.toMatch(/ on[a-z]+=/)
    const m = JSON.parse(read('manifest.json'))
    // @5 reads no Thread itself: K2's Conversation beside it does.
    expect(m.caps).toEqual(
      dir === 'kanban-5'
        ? ['agents:read', 'thread:post', 'store:read', 'store:write']
        : ['agents:read', 'thread:read', 'thread:post', 'store:read', 'store:write'],
    )
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

  it('makes its toolbar buttons holes in K2’s top band', async () => {
    const f = fakeK2({ data: THREE() })
    await started(f)
    expect(f.hitRegions.length).toBeGreaterThan(0)
    expect(Array.isArray(f.hitRegions[0])).toBe(true)
  })
})

// ── @1–@4: the board's own Thread panel, behind its Thread button ────────

describe.each(['kanban-1', 'kanban-2', 'kanban-3', 'kanban-4'] as const)('k2:kanban own Thread panel (%s)', (dir) => {
  beforeAll(() => useVersion(dir))

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
})

// ── k2:kanban@5: the board beside K2's own Thread ───────────────────────

/** kanban-5.toml's page: the board, then the folded "Thread" column. */
const PAGE_COLUMNS = (): LayoutColumn[] => [
  { index: 0, label: 'Column 1', collapsible: false, collapsed: false },
  { index: 1, label: 'Thread', collapsible: true, collapsed: true },
]

/** agents.subscribe rows from the board's view (the Garden's reach), the
 *  one K2's Conversation shows marked `selected`. */
const reachRows = (selected: string | null): Array<Record<string, unknown>> =>
  ['cortana::local', 'scout::local', 'ops::box.k2.dev'].map((address) => ({
    address,
    label: address.split('::')[0],
    state: 'ok',
    selected: address === selected,
  }))

describe('k2:kanban@5: the Thread is K2’s own Conversation in its column', () => {
  beforeAll(() => useVersion('kanban-5'))

  it('redraws no Thread: no panel, no Thread reads, the old panel’s look gone', async () => {
    const f = fakeK2({ data: THREE(), columns: PAGE_COLUMNS() })
    const m = await started(f)
    await until('agents read', () => f.agentSubs.length === 1)
    f.agentSubs[0](reachRows('cortana::local'))
    expect(f.threadSubs).toEqual([])
    expect(m.doc.querySelector('#threadDock, #dockBody, .ap-trigger, .dock-compose, .bubble')).toBeNull()
    for (const gone of ['picker.js', 'dock.js']) expect(HTML).not.toContain(gone)
    expect(read('kanban.css')).not.toMatch(/\.thread-dock|\.bubble|\.dock-|\.ap-|--kb-bubble/)
    expect(m.win.KanbanThread.look()).toBe('companion')
    // look = "garden" still follows the Garden theme's colours.
    const g = fakeK2({ data: THREE(), columns: PAGE_COLUMNS(), config: { look: 'garden' } })
    const n = await started(g)
    expect(n.win.KanbanThread.look()).toBe('garden')
  })

  it('Thread folds and unfolds the "Thread" column, and shows its state', async () => {
    const f = fakeK2({ data: THREE(), columns: PAGE_COLUMNS() })
    const m = await started(f)
    const btn = m.$<HTMLButtonElement>('#btnThread')
    await until('Thread button', () => !btn.hidden)
    expect(m.win.KanbanThread.column()).toBe(1)
    expect(btn.getAttribute('aria-pressed')).toBe('false')
    btn.click()
    await until('unfolded', () => btn.getAttribute('aria-pressed') === 'true')
    expect(f.toggles).toEqual([1])
    btn.click()
    await until('folded', () => btn.getAttribute('aria-pressed') === 'false')
    expect(f.toggles).toEqual([1, 1])
    // K2's own tab opens it: the button follows.
    f.fold(1, false)
    expect(btn.getAttribute('aria-pressed')).toBe('true')

    // The label wins over order; any collapsible column is the fallback.
    const named = fakeK2({
      data: THREE(),
      columns: [
        { index: 0, label: 'Notes', collapsible: true, collapsed: false },
        { index: 1, label: 'Column 2', collapsible: false, collapsed: false },
        { index: 2, label: 'Thread', collapsible: true, collapsed: true },
      ],
    })
    const n = await started(named)
    await until('Thread button', () => !n.$('#btnThread').hidden)
    expect(n.win.KanbanThread.column()).toBe(2)

    // A page with no collapsible column (the board alone): no Thread button.
    const alone = fakeK2({ data: THREE(), columns: [{ index: 0, label: 'Column 1', collapsible: false, collapsed: false }] })
    const a = await started(alone)
    await until('layout read', () => a.win.KanbanThread.column() === -1 && alone.layoutSubs.length === 1)
    expect(a.$('#btnThread').hidden).toBe(true)
  })

  it('Send board posts the board to the agent the Thread’s header shows, and follows a new pick', async () => {
    const f = fakeK2({ data: THREE(), columns: PAGE_COLUMNS() })
    const m = await started(f)
    const send = m.$<HTMLButtonElement>('#btnSend')
    await until('agents read', () => f.agentSubs.length === 1)
    // Nobody shown yet: Send board waits for a pick in the header.
    f.agentSubs[0](reachRows(null))
    expect(send.hidden).toBe(false)
    expect(send.disabled).toBe(true)
    expect(send.title).toBe('Pick an agent in the Thread first')

    // The header picks Cortana (the shared selection): Send board posts to her.
    f.agentSubs[0](reachRows('cortana::local'))
    expect(send.disabled).toBe(false)
    expect(send.title).toBe('Post the board, as markdown, to cortana in the Thread')
    send.click()
    await until('posted', () => f.posts.length === 1)
    expect(f.posts[0][0]).toBe('cortana::local')
    expect(f.posts[0][1]).toContain('## To do')
    expect(f.posts[0][1]).toContain('Alpha')
    await until('toast', () => m.$('#toast').textContent === 'Sent the board to cortana')

    // The header switches to another server's agent: the next send goes there.
    f.agentSubs[0](reachRows('ops::box.k2.dev'))
    expect(m.win.KanbanThread.shown()).toBe('ops::box.k2.dev')
    send.click()
    await until('posted again', () => f.posts.length === 2)
    expect(f.posts[1][0]).toBe('ops::box.k2.dev')
    // The board never picks for the header.
    expect(f.convOpens).toEqual([])

    // A refused post says so.
    const bad = fakeK2({ data: THREE(), columns: PAGE_COLUMNS(), postError: 'sending_off' })
    const b = await started(bad)
    await until('agents read', () => bad.agentSubs.length === 1)
    bad.agentSubs[0](reachRows('scout::local'))
    b.$('#btnSend').click()
    await until('refused', () => (b.$('#toast').textContent ?? '').startsWith('Send failed: posting is paused'))

    // Without thread.post there is no Send board.
    const ro = fakeK2({ data: THREE(), columns: PAGE_COLUMNS(), caps: ALL_CAPS.filter((c) => c !== 'thread.post') })
    const r = await started(ro)
    await until('agents read', () => ro.agentSubs.length === 1)
    ro.agentSubs[0](reachRows('scout::local'))
    expect(r.$('#btnSend').hidden).toBe(true)
  })

  it('carries @4’s picked agent over once, only while the Thread shows no one', async () => {
    const saves = new Map<string, Json>([['dock/g-test0001', { open: true, agent: 'scout::local' }]])
    const f = fakeK2({ data: THREE(), columns: PAGE_COLUMNS(), saves })
    await started(f)
    await until('agents read', () => f.agentSubs.length === 1)
    f.agentSubs[0](reachRows(null))
    await until('removed', () => f.saveRemoves.length === 1)
    expect(f.convOpens).toEqual(['scout::local'])
    expect(f.saveRemoves).toEqual(['dock/g-test0001'])
    // Never again, even if the Thread shows no one later.
    f.agentSubs[0](reachRows(null))
    await tick()
    expect(f.convOpens).toEqual(['scout::local'])

    // The Thread already shows someone (the header's own pick): nothing
    // moves, and the old save waits.
    const kept = new Map<string, Json>([['dock/g-test0001', { open: false, agent: 'scout::local' }]])
    const g = fakeK2({ data: THREE(), columns: PAGE_COLUMNS(), saves: kept })
    await started(g)
    await until('agents read', () => g.agentSubs.length === 1)
    g.agentSubs[0](reachRows('cortana::local'))
    await tick()
    expect(g.convOpens).toEqual([])
    expect(g.saveRemoves).toEqual([])
  })
})

// ── k2:kanban@2: the Thread panel's message flow and look ───────────────

const SCOUT = 'scout::local'
const nowSec = (): number => Math.floor(Date.now() / 1000)

interface ViewItem {
  seq: number
  id: string
  doc: Record<string, unknown>
}

function textItem(seq: number, body: string, o: { mine?: boolean; from?: string; ago?: number } = {}): ViewItem {
  return {
    seq,
    id: `m${seq}`,
    doc: {
      id: `m${seq}`,
      kind: 'text',
      from: o.from ?? (o.mine ? 'Rosson' : SCOUT),
      to: null,
      created_at: nowSec() - (o.ago ?? 0),
      body,
      via: o.mine ? 'compose' : null,
      choice: null,
    },
  }
}

function view(items: ViewItem[], turn: { state: string; since: number } | null = null) {
  return { address: SCOUT, phase: 'live', note: null, items, hasMore: false, turn }
}

/** A board with the Thread panel open on scout (saved per Garden). */
async function openOnScout(
  o: { row?: Record<string, unknown>; postError?: string; config?: Record<string, unknown>; k2?: Parameters<typeof fakeK2>[0] } = {},
) {
  const saves = new Map<string, Json>([['dock/g-test0001', { open: true, agent: SCOUT }]])
  const f = fakeK2({ data: THREE(), saves, postError: o.postError, config: o.config, ...o.k2 })
  const m = await started(f)
  await until('agents read', () => f.agentSubs.length === 1)
  f.agentSubs[0]([{ address: SCOUT, label: 'scout', state: 'ok', working: false, needsYou: false, activity: 'idle', counts: null, ...o.row }])
  await until('Thread open', () => f.threadSubs.length === 1)
  expect(f.threadSubs[0].address).toBe(SCOUT)
  return { f, m }
}

describe.each(['kanban-2', 'kanban-3', 'kanban-4'] as const)('k2:kanban Thread panel (%s; the K2 Thread look in K2 Companion colours)', (dir) => {
  beforeAll(() => useVersion(dir))

  it('draws bubbles: yours right and the agent’s left, "author · time" under each, markdown inside, safely', async () => {
    const { f, m } = await openOnScout()
    f.threadSubs[0].cb(
      view([
        textItem(1, 'Which database should the **ledger** use?\n\n- `SQLite`: one file\n- `Postgres`: a server', { ago: 7200 }),
        textItem(2, 'What would the migration path look like?', { mine: true, ago: 900 }),
        textItem(3, 'A dump and load. <img src=x onerror=alert(1)> [docs](https://example.com/x)', { ago: 600 }),
      ]),
    )
    const msgs = [...m.doc.querySelectorAll<HTMLElement>('#dockBody .msg')]
    expect(msgs.map((n) => n.className)).toEqual(['msg theirs', 'msg mine', 'msg theirs'])
    expect(msgs.map((n) => n.getAttribute('data-seq'))).toEqual(['1', '2', '3'])
    // Each message: a bubble, then its "author · time" line.
    for (const n of msgs) expect([...n.children].map((c) => c.className.split(' ')[0])).toEqual(['bubble', 'meta'])
    expect(msgs[0].querySelector('.bubble')!.className).toBe('bubble bubble-agent')
    expect(msgs[1].querySelector('.bubble')!.className).toBe('bubble bubble-me')
    expect(msgs.map((n) => n.querySelector('.meta')!.textContent)).toEqual(['scout · 2h ago', 'You · 15m ago', 'scout · 10m ago'])
    // Markdown, built as nodes: bold, a list, inline code.
    const first = msgs[0].querySelector('.body')!
    expect(first.classList.contains('md')).toBe(true)
    expect(first.querySelector('strong')!.textContent).toBe('ledger')
    expect([...first.querySelectorAll('li code')].map((c) => c.textContent)).toEqual(['SQLite', 'Postgres'])
    // Raw HTML stays text; a link shows its URL, never an <a> or <img>.
    const third = msgs[2].querySelector('.body')!
    expect(third.querySelector('img, a, script')).toBeNull()
    expect(third.textContent).toContain('<img src=x onerror=alert(1)>')
    expect(third.querySelector('.md-url')!.textContent).toBe('https://example.com/x')
    // The empty Thread says what K2's does.
    f.threadSubs[0].cb(view([]))
    expect(m.$('#dockBody').textContent).toBe('Message scout. It reads this like a text.')
  })

  it('Enter sends and Shift+Enter is a new line; the round Send lights up; the post reaches the agent and its reply renders', async () => {
    const { f, m } = await openOnScout()
    f.threadSubs[0].cb(view([textItem(1, 'Ready when you are.', { ago: 60 })]))
    const text = m.$<HTMLTextAreaElement>('#dockText')
    const send = m.$<HTMLButtonElement>('#dockSendBtn')
    expect(text.placeholder).toBe('Message scout')
    expect(send.getAttribute('aria-label')).toBe('Send')
    expect(send.title).toBe('Send (Enter)')
    expect(send.disabled).toBe(true)
    expect(send.classList.contains('ready')).toBe(false)
    m.type(text, 'Take the Doing column')
    expect(send.disabled).toBe(false)
    expect(send.classList.contains('ready')).toBe(true)

    // Shift+Enter: a new line, no post.
    const shift = new m.win.KeyboardEvent('keydown', { key: 'Enter', shiftKey: true, bubbles: true, cancelable: true })
    text.dispatchEvent(shift)
    expect(shift.defaultPrevented).toBe(false)
    await tick()
    expect(f.posts).toEqual([])

    // Enter: one post to the picked agent; the field clears once K2 took it.
    const enter = new m.win.KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })
    text.dispatchEvent(enter)
    expect(enter.defaultPrevented).toBe(true)
    await until('posted', () => f.posts.length === 1)
    expect(f.posts[0]).toEqual([SCOUT, 'Take the Doing column'])
    await until('cleared', () => text.value === '')
    expect(send.disabled).toBe(true)
    // Until the agent answers, the ear says it was sent.
    const ear = m.$('#dockTurn')
    expect(ear.hidden).toBe(false)
    expect(ear.dataset.tone).toBe('pending')
    expect(ear.querySelector('.ear-label')!.textContent).toBe('Sent · waiting…')

    // The round Send posts too.
    m.type(text, 'And the Done one')
    send.click()
    await until('posted twice', () => f.posts.length === 2)
    expect(f.posts[1]).toEqual([SCOUT, 'And the Done one'])

    // Your post comes back, then the agent's reply: both render, the reply
    // on the left, and the ear goes.
    f.threadSubs[0].cb(
      view([
        textItem(1, 'Ready when you are.', { ago: 60 }),
        textItem(101, 'Take the Doing column', { mine: true }),
        textItem(102, 'And the Done one', { mine: true }),
      ]),
    )
    expect(ear.hidden).toBe(false)
    f.threadSubs[0].cb(
      view([
        textItem(1, 'Ready when you are.', { ago: 60 }),
        textItem(101, 'Take the Doing column', { mine: true }),
        textItem(102, 'And the Done one', { mine: true }),
        textItem(103, 'On it: **Doing** and Done are mine.'),
      ]),
    )
    const last = [...m.doc.querySelectorAll<HTMLElement>('#dockBody .msg')].at(-1)!
    expect(last.className).toBe('msg theirs')
    expect(last.querySelector('.bubble strong')!.textContent).toBe('Doing')
    expect(last.querySelector('.meta')!.textContent).toBe('scout · just now')
    expect(ear.hidden).toBe(true)
  })

  it('a refused post keeps your text and says why', async () => {
    const { f, m } = await openOnScout({ postError: 'sending_off' })
    f.threadSubs[0].cb(view([]))
    const text = m.$<HTMLTextAreaElement>('#dockText')
    m.type(text, 'Hello?')
    m.$<HTMLButtonElement>('#dockSendBtn').click()
    await until('refused', () => m.doc.querySelector('.dock-err') !== null)
    expect(m.$('.dock-err').textContent).toBe('post failed: posting is paused; click Resume on the widget')
    expect(text.value).toBe('Hello?')
    expect(m.$<HTMLButtonElement>('#dockSendBtn').disabled).toBe(false)
    expect(m.$('#dockTurn').hidden).toBe(true)
  })

  it('the working ear: a dot, the state, the counters and the turn’s clock', async () => {
    const counts = { subagents: 1, tools: 4, commands: 3 }
    const { f, m } = await openOnScout({ row: { working: true, activity: 'working', counts } })
    f.threadSubs[0].cb(view([textItem(1, 'Hi', { mine: true, ago: 80 })], { state: 'working', since: Date.now() - 74_000 }))
    const ear = m.$('#dockTurn')
    expect(ear.hidden).toBe(false)
    expect(ear.dataset.tone).toBe('working')
    expect([...ear.children].map((c) => c.className)).toEqual(['ear-dot', 'ear-label', 'ear-counts', 'ear-clock'])
    expect(ear.querySelector('.ear-label')!.textContent).toBe('Working')
    const tags = [...ear.querySelectorAll<HTMLElement>('.ear-tag')]
    expect(tags.map((t) => [t.dataset.count, t.textContent, t.title])).toEqual([
      ['subagents', '1', '1 subagent'],
      ['tools', '4', '4 tool calls'],
      ['commands', '3', '3 commands'],
    ])
    for (const t of tags) expect(t.querySelector('svg path')).not.toBeNull()
    expect(ear.querySelector('.ear-clock')!.textContent).toMatch(/^1:1[45]$/)
    expect(ear.getAttribute('aria-label')).toMatch(/^Working, 1:1[45], 1 subagent, 4 tool calls, 3 commands$/)
    // Needs you.
    f.threadSubs[0].cb(view([], { state: 'needs-you', since: Date.now() - 5_000 }))
    expect(ear.dataset.tone).toBe('needs-you')
    expect(ear.querySelector('.ear-label')!.textContent).toBe('Needs you')
    expect(ear.querySelector('.ear-clock')!.textContent).toMatch(/^0:0[56]$/)
    // Idle: no ear.
    f.agentSubs[0]([{ address: SCOUT, label: 'scout', state: 'ok', working: false, needsYou: false, activity: 'idle', counts: null }])
    f.threadSubs[0].cb(view([], null))
    expect(ear.hidden).toBe(true)
  })

  it('a choice card answers inside the agent’s bubble; a secret card points to K2’s own Thread', async () => {
    const { f, m } = await openOnScout()
    f.threadSubs[0].cb(
      view([
        {
          seq: 1,
          id: 'c1',
          doc: {
            id: 'c1',
            kind: 'choice',
            from: SCOUT,
            created_at: nowSec() - 120,
            body: null,
            via: null,
            choice: { prompt: 'Start on **SQLite**?', options: [{ label: 'SQLite' }, { label: 'Postgres' }], allow_custom: false, status: 'pending', answer: null },
          },
        },
        { seq: 2, id: 's1', doc: { id: 's1', kind: 'secret' } },
      ]),
    )
    const card = m.$('.msg[data-msg-id="c1"]')
    expect(card.className).toBe('msg theirs')
    expect(card.querySelector('.bubble .body strong')!.textContent).toBe('SQLite')
    const chips = [...card.querySelectorAll<HTMLButtonElement>('.bubble .choice-opts .chip')]
    expect(chips.map((c) => c.textContent)).toEqual(['SQLite', 'Postgres'])
    chips[1].click()
    await until('answered', () => f.answers.length === 1)
    expect(f.answers[0]).toEqual([SCOUT, 'c1', 'Postgres'])
    expect(m.$('.msg[data-msg-id="s1"] .bubble .status').textContent).toContain('K2’s own Thread')
  })

  it('wears K2 Companion’s look by default; look = "garden" follows the Garden theme instead', async () => {
    const a = await openOnScout()
    expect(a.m.win.KanbanDock.look()).toBe('companion')
    expect(a.m.doc.documentElement.classList.contains('look-garden')).toBe(false)
    const b = await openOnScout({ config: { look: 'garden' } })
    await until('garden look', () => b.m.doc.documentElement.classList.contains('look-garden'))
    expect(b.m.win.KanbanDock.look()).toBe('garden')
    // The look never changes what it does: still talks to scout.
    expect(b.m.win.KanbanDock.address()).toBe(SCOUT)
  })
})

// ── k2:kanban@3: the Thread panel never moves a reader who scrolled up ──
// Rosson 2026-10-20: "When new messages come in it should just offer to see
// those new messages down below, not jump me all over the chat." @2 rebuilt
// the whole list on every Thread event, and WebKit (the macOS window) puts a
// scroll box whose content was replaced back at the top. jsdom has no
// layout, so the dock's list gets one: 300 px tall, 60 px per message (120
// more for one marked GROWN), and replacing its content resets scrollTop to
// 0, as WebKit does.

const VIEW_PX = 300
const ROW_PX = 60

function layoutDock(m: Awaited<ReturnType<typeof started>>) {
  const { win } = m
  const body = m.$('#dockBody')
  const msgs = (): HTMLElement[] => [...body.querySelectorAll<HTMLElement>('.msg')]
  const height = (n: HTMLElement): number => ROW_PX + ((n.textContent ?? '').includes('GROWN') ? 120 : 0)
  const total = (): number => msgs().reduce((a, n) => a + height(n), 0)
  let top = 0
  const max = (): number => Math.max(0, total() - VIEW_PX)
  Object.defineProperty(body, 'clientHeight', { configurable: true, get: () => VIEW_PX })
  Object.defineProperty(body, 'scrollHeight', { configurable: true, get: () => Math.max(VIEW_PX, total()) })
  Object.defineProperty(body, 'scrollTop', {
    configurable: true,
    get: () => top,
    set: (v: number) => {
      top = Math.min(Math.max(0, v), max())
    },
  })
  const glides: number[] = []
  ;(body as unknown as { scrollTo: (o: { top: number }) => void }).scrollTo = (o) => {
    glides.push(o.top)
    body.scrollTop = o.top
  }
  const replace = body.replaceChildren.bind(body)
  body.replaceChildren = (...nodes: Array<Node | string>) => {
    replace(...nodes)
    top = 0 // WebKit: new content starts at the top
  }
  const rect = (t: number, h: number): DOMRect => ({ top: t, bottom: t + h, left: 0, right: 300, width: 300, height: h, x: 0, y: t, toJSON: () => ({}) }) as DOMRect
  const original = win.HTMLElement.prototype.getBoundingClientRect
  win.HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
    if (this === body) return rect(0, VIEW_PX)
    if (this.classList.contains('msg') && this.parentElement?.parentElement === body) {
      let y = -top
      for (const n of msgs()) {
        if (n === this) return rect(y, height(n))
        y += height(n)
      }
    }
    return original.call(this)
  }
  /** The person scrolls (a wheel, then the scroll it makes). */
  const scrollTo = (to: number): void => {
    body.dispatchEvent(new win.Event('wheel'))
    body.scrollTop = to
    body.dispatchEvent(new win.Event('scroll'))
  }
  const firstVisible = (): { id: string | null; at: number } => {
    const n = msgs().find((x) => x.getBoundingClientRect().bottom > 0)!
    return { id: n.getAttribute('data-msg-id'), at: n.getBoundingClientRect().top }
  }
  return { body, msgs, max, glides, scrollTo, firstVisible, gap: () => max() - top }
}

const lines = (n: number, o: { grown?: number } = {}): ViewItem[] =>
  Array.from({ length: n }, (_, i) => textItem(i + 1, i + 1 === o.grown ? `GROWN line ${i + 1}` : `line ${i + 1}`, { ago: 900 - i }))

describe.each(['kanban-3', 'kanban-4'] as const)('k2:kanban Thread panel scrolling (%s; K2’s stick-to-bottom)', (dir) => {
  beforeAll(() => useVersion(dir))

  it('a reader at the bottom stays there; one who scrolled up never moves, and a pill counts what came in', async () => {
    const { f, m } = await openOnScout()
    const d = layoutDock(m)
    const push = (items: ViewItem[], turn: { state: string; since: number } | null = null): void => f.threadSubs[0].cb(view(items, turn))
    const pill = m.$<HTMLButtonElement>('.dock-newer')

    // Opens at the newest; a new message at the bottom glides into view.
    push(lines(20))
    expect(d.gap()).toBe(0)
    push(lines(21))
    expect(d.gap()).toBe(0)
    expect(d.glides.at(-1)).toBe(d.max())
    expect(pill.hidden).toBe(true)

    // Scrolled up: a new message leaves the reader exactly where they are,
    // keeps every message they were reading (no rebuild), and says so below.
    d.scrollTo(240)
    const reading = d.msgs()
    const before = d.firstVisible()
    push(lines(22))
    expect(m.$('#dockBody').scrollTop).toBe(240)
    expect(d.msgs().slice(0, reading.length)).toEqual(reading)
    expect(d.firstVisible()).toEqual(before)
    expect(pill.hidden).toBe(false)
    expect(pill.textContent).toBe('1 new message ↓')
    push(lines(23))
    expect(m.$('#dockBody').scrollTop).toBe(240)
    expect(pill.textContent).toBe('2 new messages ↓')

    // An earlier message re-renders taller (an edit, a card answered): the
    // message being read stays put on screen.
    push(lines(23, { grown: 1 }))
    expect(d.firstVisible()).toEqual(before)
    // The working tab comes and goes: nothing moves.
    push(lines(23, { grown: 1 }), { state: 'working', since: Date.now() })
    push(lines(23, { grown: 1 }), null)
    expect(d.firstVisible()).toEqual(before)
    expect(pill.textContent).toBe('2 new messages ↓')

    // Scrolling to the bottom by hand clears the pill; from there, new
    // messages keep them at the bottom again.
    d.scrollTo(d.max())
    expect(pill.hidden).toBe(true)
    push(lines(24, { grown: 1 }))
    expect(d.gap()).toBe(0)
    expect(pill.hidden).toBe(true)
  })

  it('the pill goes to the newest and clears; your own message always goes to the bottom', async () => {
    const { f, m } = await openOnScout()
    const d = layoutDock(m)
    const push = (items: ViewItem[]): void => f.threadSubs[0].cb(view(items))
    const pill = m.$<HTMLButtonElement>('.dock-newer')
    push(lines(20))
    d.scrollTo(0)
    push(lines(22))
    expect(m.$('#dockBody').scrollTop).toBe(0)
    expect(pill.textContent).toBe('2 new messages ↓')
    pill.click()
    expect(d.gap()).toBe(0)
    expect(pill.hidden).toBe(true)

    // Scrolled up, you send: the list goes to the bottom, and your own
    // message coming back is no "new message".
    d.scrollTo(60)
    const text = m.$<HTMLTextAreaElement>('#dockText')
    m.type(text, 'Take the Doing column')
    text.dispatchEvent(new m.win.KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }))
    expect(d.gap()).toBe(0)
    await until('posted', () => f.posts.length === 1)
    push([...lines(22), textItem(101, 'Take the Doing column', { mine: true })])
    expect(d.gap()).toBe(0)
    expect(pill.hidden).toBe(true)
  })
})

// ── k2:kanban@4: Thread parity (Rosson 2026-10-20, for 0.46.3) ──────────
// "app gateway and garden gateway both need the attachments and any other
// systems to match parity with the new thread features." The panel draws a
// message's files (chips with their state; images as pictures, only ever
// through <img src="data:…">), links printed paths K2 says lead into the
// workspace, takes files in the message box (K2's picker), marks the
// Thread read at the newest message, catches up with a delta read and
// pages back. On an older K2 (no such verbs) it is @3's panel.

const PNG_B64 = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='

function fileItem(seq: number, body: string, attachments: Array<Record<string, unknown>>, o: { mine?: boolean } = {}): ViewItem {
  const it = textItem(seq, body, o)
  it.doc.attachments = attachments
  return it
}

describe('k2:kanban@4 Thread panel: files, links, the message box’s picks, read state', () => {
  beforeAll(() => useVersion('kanban-4'))

  it('draws a message’s files: chips with their state, a hidden one without a name, pictures through <img> data: URLs only', async () => {
    const { f, m } = await openOnScout({
      k2: { thumbs: { 'm1/0': { mime: 'image/png', base64: PNG_B64 }, 'm1/4': { mime: 'text/html', base64: 'PHNjcmlwdD4=' } } },
    })
    f.threadSubs[0].cb(
      view([
        fileItem(1, 'Here are the files.', [
          { kind: 'file', state: 'ok', name: 'shot.png', mime: 'image/png', size: 2048 },
          { kind: 'file', state: 'changed', name: 'plan.md', size: 900, line: 4 },
          { kind: 'file', state: 'gone', name: 'old.txt' },
          { kind: 'file', state: 'hidden' },
          { kind: 'file', state: 'ok', name: 'weird.svg', mime: 'image/svg+xml' },
          { kind: 'file', state: 'ok', name: 'photo.heic' },
        ]),
        fileItem(2, '', [{ kind: 'file', state: 'ok', name: 'notes.txt', size: 12 }], { mine: true }),
      ]),
    )
    const msg = m.$('.msg[data-msg-id="m1"]')
    // Under the bubble, before "author · time".
    expect([...msg.children].map((c) => c.className.split(' ')[0])).toEqual(['bubble', 'msg-files', 'meta'])
    const files = [...msg.querySelectorAll<HTMLElement>('.msg-files > *')]
    expect(files.map((n) => n.getAttribute('data-file-state'))).toEqual(['ok', 'changed', 'gone', 'hidden', 'ok', 'ok'])
    expect(files[1].textContent).toBe('plan.md:4changed since sent')
    expect(files[2].textContent).toBe('old.txtmoved or deleted')
    expect((files[2] as HTMLButtonElement).disabled).toBe(true)
    expect(files[3].textContent).toBe('A file (not shared with you)')
    // Every image asks K2 for its 512 px thumbnail.
    await until('thumbs asked', () => f.attachmentCalls.length === 3)
    expect(f.attachmentCalls).toEqual([
      [SCOUT, 'm1', 0, { mode: 'thumb', px: 512 }],
      [SCOUT, 'm1', 4, { mode: 'thumb', px: 512 }],
      [SCOUT, 'm1', 5, { mode: 'thumb', px: 512 }],
    ])
    await until('picture drawn', () => msg.querySelector('.file-thumb img') !== null)
    const img = msg.querySelector<HTMLImageElement>('.file-thumb img')!
    expect(img.getAttribute('src')).toBe(`data:image/png;base64,${PNG_B64}`)
    expect(img.alt).toBe('shot.png')
    // A thumbnail that isn't an image type, or none at all, falls back to the chip.
    await until('fallbacks', () => msg.querySelectorAll('.file-thumb').length === 1)
    expect([...msg.querySelectorAll('.file-chip')].map((n) => n.querySelector('.file-name')!.textContent)).toEqual([
      'plan.md:4',
      'old.txt',
      'A file (not shared with you)',
      'weird.svg',
      'photo.heic',
    ])
    // Nothing a file holds is ever markup in the panel.
    expect(msg.querySelector('script, svg, iframe, object')).toBeNull()

    // A click opens it in K2 (by message and index; never a path).
    ;(msg.querySelector('.file-thumb') as HTMLElement).click()
    ;(files[1] as HTMLElement).click()
    files[2].click()
    await until('opened twice', () => f.opens.length === 2)
    expect(f.opens).toEqual([
      [SCOUT, { docId: 'm1', n: 0 }],
      [SCOUT, { docId: 'm1', n: 1 }],
    ])

    // Files alone are a message: no empty bubble.
    const alone = m.$('.msg[data-msg-id="m2"]')
    expect([...alone.children].map((c) => c.className.split(' ')[0])).toEqual(['msg-files', 'meta'])
  })

  it('links printed paths K2 says lead into the workspace; a click opens the file at its line', async () => {
    const { f, m } = await openOnScout({ k2: { linkable: { 'src/app.ts:12': 'src/app.ts', 'Cargo.toml': 'Cargo.toml' } } })
    f.threadSubs[0].cb(
      view([
        textItem(
          1,
          'Changed src/app.ts:12 and `Cargo.toml`, not lib/gone.rs, see https://example.com/a/b.ts and and/or.\n\n```\nsrc/in/fence.ts\n```',
          { ago: 60 },
        ),
      ]),
    )
    await until('links asked', () => f.linkCalls.length === 1)
    // Path-shaped text only: never a URL, a fenced block or prose like and/or.
    expect(f.linkCalls[0]).toEqual([SCOUT, ['src/app.ts:12', 'Cargo.toml', 'lib/gone.rs']])
    await until('links drawn', () => m.doc.querySelectorAll('.file-link').length === 2)
    const links = [...m.doc.querySelectorAll<HTMLElement>('.file-link')]
    expect(links.map((l) => [l.textContent, l.getAttribute('role'), l.getAttribute('data-rel')])).toEqual([
      ['src/app.ts:12', 'link', 'src/app.ts'],
      ['Cargo.toml', 'link', 'Cargo.toml'],
    ])
    // Not in the workspace: plain text.
    expect(m.doc.querySelector('.file-cand[data-cand="lib/gone.rs"]')!.classList.contains('file-link')).toBe(false)
    links[0].click()
    links[1].dispatchEvent(new m.win.KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }))
    await until('opened', () => f.opens.length === 2)
    expect(f.opens).toEqual([
      [SCOUT, { rel: 'src/app.ts', line: 12 }],
      [SCOUT, { rel: 'Cargo.toml' }],
    ])
    // The same text again is answered from what K2 said: no second ask.
    f.threadSubs[0].cb(view([textItem(1, 'Changed src/app.ts:12', { ago: 60 }), textItem(2, 'Also src/app.ts:12', { ago: 30 })]))
    expect(m.doc.querySelectorAll('.file-link').length).toBe(2)
    await tick()
    expect(f.linkCalls).toHaveLength(1)
  })

  it('the message box takes files: + opens K2’s picker, chips wait (removable), and files alone send', async () => {
    const { f, m } = await openOnScout({
      k2: {
        picks: [
          { files: [{ relPath: 'shots/a.png', name: 'a.png' }, { relPath: '.k2/downloads/b.pdf', name: 'b.pdf' }] },
          { files: [] },
        ],
      },
    })
    f.threadSubs[0].cb(view([]))
    const plus = m.$<HTMLButtonElement>('.attach-round')
    const send = m.$<HTMLButtonElement>('#dockSendBtn')
    expect(plus.hidden).toBe(false)
    expect(plus.getAttribute('aria-label')).toBe('Attach files')
    expect(send.disabled).toBe(true)
    plus.click()
    await until('picked', () => m.doc.querySelectorAll('.compose-file').length === 2)
    expect(f.pickCalls).toEqual([SCOUT])
    expect([...m.doc.querySelectorAll('.compose-file .file-name')].map((n) => n.textContent)).toEqual(['a.png', 'b.pdf'])
    expect(send.disabled).toBe(false)
    // Remove one; a cancelled pick adds nothing.
    m.$<HTMLButtonElement>('.compose-file[data-rel=".k2/downloads/b.pdf"] button').click()
    expect(m.doc.querySelectorAll('.compose-file').length).toBe(1)
    plus.click()
    await until('second pick', () => f.pickCalls.length === 2)
    await tick()
    expect(m.doc.querySelectorAll('.compose-file').length).toBe(1)
    // Files alone are a message.
    send.click()
    await until('posted', () => f.posts.length === 1)
    expect(f.posts[0]).toEqual([SCOUT, '', { attachments: [{ relPath: 'shots/a.png' }] }])
    await until('chips cleared', () => m.doc.querySelectorAll('.compose-file').length === 0)
    // Text with no files is posted as before (two arguments).
    m.type(m.$<HTMLTextAreaElement>('#dockText'), 'Thanks')
    send.click()
    await until('posted twice', () => f.posts.length === 2)
    expect(f.posts[1]).toEqual([SCOUT, 'Thanks'])
  })

  it('marks the Thread read at the newest message while it is shown, once per newer message', async () => {
    const { f, m } = await openOnScout()
    f.threadSubs[0].cb(view([textItem(1, 'one'), textItem(2, 'two')]))
    await until('read', () => f.markReads.length === 1)
    expect(f.markReads[0]).toEqual([SCOUT, { seq: 2 }])
    f.threadSubs[0].cb(view([textItem(1, 'one'), textItem(2, 'two')], { state: 'working', since: Date.now() }))
    await tick()
    expect(f.markReads).toHaveLength(1)
    // The window hidden: nothing is read until it shows again.
    let state = 'hidden'
    Object.defineProperty(m.doc, 'visibilityState', { configurable: true, get: () => state })
    f.threadSubs[0].cb(view([textItem(1, 'one'), textItem(2, 'two'), textItem(3, 'three')]))
    await tick()
    expect(f.markReads).toHaveLength(1)
    state = 'visible'
    m.doc.dispatchEvent(new m.win.Event('visibilitychange'))
    await until('read again', () => f.markReads.length === 2)
    expect(f.markReads[1]).toEqual([SCOUT, { seq: 3 }])
  })

  it('catches up with a delta when the window shows again: new items join, changed ones update, removed ones go', async () => {
    const { f, m } = await openOnScout({
      k2: {
        reads: (o) =>
          o.afterSeq === 3
            ? {
                items: [textItem(4, 'four, missed while away')],
                hasMore: false,
                changed: [textItem(2, 'two, edited')],
                removed: [{ id: 'm1', seq: 1 }],
              }
            : { items: [], hasMore: false, changed: [], removed: [] },
      },
    })
    f.threadSubs[0].cb(view([textItem(1, 'one'), textItem(2, 'two'), textItem(3, 'three')]))
    m.doc.dispatchEvent(new m.win.Event('visibilitychange'))
    await until('caught up', () => m.doc.querySelectorAll('#dockBody .msg').length === 3)
    expect(f.reads[0]).toEqual([SCOUT, { afterSeq: 3 }])
    expect([...m.doc.querySelectorAll('#dockBody .msg')].map((n) => n.querySelector('.body')!.textContent!.trim())).toEqual([
      'two, edited',
      'three',
      'four, missed while away',
    ])
  })

  it('scrolled to the top with earlier messages left, it loads them above without moving you or counting them as new', async () => {
    const { f, m } = await openOnScout({
      k2: { reads: (o) => (o.beforeSeq === 11 ? { items: lines(10), hasMore: false } : { items: [], hasMore: false }) },
    })
    const d = layoutDock(m)
    const later = Array.from({ length: 10 }, (_, i) => textItem(i + 11, `line ${i + 11}`, { ago: 800 - i }))
    f.threadSubs[0].cb({ ...view(later), hasMore: true })
    const pill = m.$<HTMLButtonElement>('.dock-newer')
    d.scrollTo(0)
    await until('earlier read', () => f.reads.length === 1)
    expect(f.reads[0]).toEqual([SCOUT, { beforeSeq: 11 }])
    await until('earlier drawn', () => d.msgs().length === 20)
    expect(d.msgs()[0].getAttribute('data-seq')).toBe('1')
    // What you were reading (line 11) is still at the top of the view.
    expect(d.firstVisible()).toEqual({ id: 'm11', at: 0 })
    expect(pill.hidden).toBe(true)
  })

  it('on an older K2 it is @3’s panel: no picker, no links, no pictures, no reads', async () => {
    const { f, m } = await openOnScout({ k2: { caps: PRE_PARITY_CAPS, linkable: { 'src/app.ts': 'src/app.ts' } } })
    f.threadSubs[0].cb(
      view([fileItem(1, 'See src/app.ts', [{ kind: 'file', state: 'ok', name: 'shot.png', mime: 'image/png' }])]),
    )
    expect(m.$('.attach-round').hidden).toBe(true)
    expect(m.doc.querySelector('.file-thumb')).toBeNull()
    const chip = m.$<HTMLButtonElement>('.file-chip')
    expect(chip.disabled).toBe(true)
    m.doc.dispatchEvent(new m.win.Event('visibilitychange'))
    await tick()
    await tick()
    expect([f.attachmentCalls, f.linkCalls, f.markReads, f.reads, f.opens]).toEqual([[], [], [], [], []])
    expect(m.doc.querySelector('.file-cand')).toBeNull()
  })
})

