// The built-in canvas, k2:canvas@1 (the Compute Garden): its shipped files, loaded in index.html's
// order into a fresh JSDOM per test, against a stand-in `k2` with the verbs it calls.
// What it pins:
//   - the canvas: the Garden's tiles in config order, each shown or hidden by the person (Tiles menu,
//     a tile's ×), kept in their private save; a tile they've never seen starts as its entry says,
//     so a later tile appears without moving the ones they hid; a hidden tile stops its timers;
//   - a tile whose verb this K2 doesn't offer says so instead of breaking;
//   - Compute load: the nodes from compute.nodes, busiest first or by name (saved), room and
//     held nodes, offline nodes folded, a failed read said in the tile's note, text never markup;
//   - Agents: every row from agents.subscribe in K2's order, picture or initials, working / needs
//     you, unread from thread.unread (this computer's agents), and a pick opens that agent in the
//     Thread column (unfolding it right after the click);
//   - the shipped files' hygiene (no markup sinks, no network, only K2 verbs).
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const { JSDOM } = createRequire(import.meta.url)('jsdom') as {
  JSDOM: new (html: string, opts: { runScripts: 'outside-only'; pretendToBeVisual: boolean }) => { window: unknown }
}

const ROOT = resolve(__dirname, '../../../..')
const DIR = resolve(ROOT, 'crates/k2-core/src/zen/builtin_widgets/canvas-1')
const read = (f: string): string => readFileSync(resolve(DIR, f), 'utf8')
const HTML = read('index.html')
const ORDER = [...HTML.matchAll(/<script src="([^"]+)"><\/script>/g)].map((m) => m[1])
const SCRIPTS = ORDER.map((f) => [f, read(f)] as const)
const CATALOG = JSON.parse(readFileSync(resolve(ROOT, 'crates/k2-core/src/contract/catalog.json'), 'utf8'))

/* eslint-disable @typescript-eslint/no-explicit-any */
type Any = any

const GB = 1073741824
const NOW = Math.floor(Date.now() / 1000)

function node(name: string, f: Any = {}): Any {
  return Object.assign(
    {
      name, os: 'linux', arch: 'x86_64', version: '0.46.8', online: true, state: 'active', why: null,
      slots: 3, running: 1, queued: 0, queueFrom: 'server',
      cpu: { freeMillis: 6000, totalMillis: 8000, idleMillis: 6500 }, mem: { freeBytes: 20 * GB, totalBytes: 32 * GB },
      load: [1, 1, 1], lastSeenAt: NOW, shared: false, servers: null,
      who: [{ label: 'k2', kind: 'workspace', running: 1, queued: 0 }],
      jobs: [{ state: 'running', workspace: 'k2', server: null, tier: 'normal', elapsedSecs: 60, blockedBy: null }],
      queueError: null,
    },
    f,
  )
}

function agent(handle: string, i: number, f: Any = {}): Any {
  return Object.assign(
    {
      address: handle + '::local', label: handle, index: i, server: null, state: 'ok', stateLabel: null, detail: null,
      working: false, needsYou: false, activity: 'idle', counts: null, openable: true, selected: false, avatar: null,
      color: '#336699', image: null, preview: { text: 'hi', at: NOW, from: handle, mine: false, seq: 1 },
    },
    f,
  )
}

const ALL_VERBS = ['save.get', 'save.set', 'canvas.setHitRegions', 'compute.nodes', 'agents.subscribe', 'conversation.open', 'thread.unread', 'layout.get', 'layout.set', 'layout.subscribe']

function fakeK2(opts: Any = {}): Any {
  const log: Any = { ready: 0, nodes: 0, unread: 0, holes: [] as Any[], opened: [] as string[], layoutSet: [] as Any[], agentsOff: 0, saves: Object.assign({}, opts.saves || {}) }
  const verbs = new Set<string>(opts.verbs || ALL_VERBS)
  let agentsCb: Any = null
  let visibleFn: Any = null
  const k2: Any = {
    connected: Promise.resolve(),
    config: opts.config || {},
    can: (v: string) => verbs.has(v),
    ready: () => { log.ready++ },
    onVisible: (fn: Any) => { visibleFn = fn; return () => {} },
    save: {
      get: async (k: string) => (k in log.saves ? JSON.parse(JSON.stringify(log.saves[k])) : null),
      set: async (k: string, v: unknown) => { log.saves[k] = JSON.parse(JSON.stringify(v)); return null },
    },
    canvas: { setHitRegions: async (r: Any) => { log.holes.push(r); return null } },
    compute: {
      nodes: async () => {
        log.nodes++
        if (opts.computeError) throw Object.assign(new Error('nope'), { code: 'cap_not_granted', message: 'compute.nodes needs compute:nodes' })
        return { at: NOW, controller: 'rosson.k2.dev', nodes: opts.nodes || [] }
      },
    },
    agents: {
      subscribe: (cb: Any) => {
        agentsCb = cb
        setTimeout(() => cb(opts.agents || []), 0)
        return () => { log.agentsOff++ }
      },
    },
    thread: { unread: async () => { log.unread++; return opts.unread || { ok: true, total: 0, byConversation: [] } } },
    conversation: { open: async (a: string) => { log.opened.push(a); return null } },
    layout: {
      get: async () => opts.layout || { columns: [] },
      set: async (c: Any) => { log.layoutSet.push(c); return null },
      subscribe: () => () => {},
    },
  }
  return { k2, log, push: (rows: Any) => agentsCb(rows), visible: (v: boolean) => visibleFn(v) }
}

interface Mounted { win: Any; doc: Any; fake: Any; C: Any }
// A second tile, registered the way a later K2's tile file would be (before start.js).
const NOTES_TILE = `window.K2Canvas.tile({ id: 'notes', name: 'Notes', description: 'A test tile.', size: 'half',
  mount: function (ctx) { ctx.body.textContent = 'notes'; window.notesStops = 0; return { stop: function () { window.notesStops++ } } } })`

function mount(opts: Any = {}): Mounted {
  const dom = new JSDOM(HTML.replace(/<script[^>]*><\/script>/g, '').replace(/<link[^>]*>/g, ''), { runScripts: 'outside-only', pretendToBeVisual: true })
  const win = dom.window as Any
  const fake = fakeK2(opts)
  win.k2 = fake.k2
  for (const [name, src] of SCRIPTS) {
    if (name === 'start.js' && opts.notes) win.eval(NOTES_TILE)
    win.eval(src)
  }
  return { win, doc: win.document, fake, C: win.K2Canvas }
}
const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms))
async function until(what: string, ok: () => boolean, ms = 3000): Promise<void> {
  const t0 = Date.now()
  while (Date.now() - t0 < ms) { if (ok()) return; await sleep(5) }
  throw new Error('timed out waiting for: ' + what)
}
async function started(opts: Any = {}): Promise<Mounted> {
  const m = mount(opts)
  await until('the canvas is ready', () => m.fake.log.ready === 1)
  return m
}
const tileEl = (m: Mounted, id: string): Any => m.doc.querySelector('[data-tile="' + id + '"].cv-tile')
const text = (e: Any): string => (e ? e.textContent : '')

describe('k2:canvas@1 shipped files', () => {
  it('loads the host, then each tile, then start.js', () => {
    expect(ORDER).toEqual(['canvas.js', 'tile-compute.js', 'tile-agents.js', 'start.js'])
  })

  it('declares exactly the caps its tiles use, each with a reason, all widget caps in the catalog', () => {
    const m = JSON.parse(read('manifest.json'))
    expect(m.caps).toEqual(['compute:nodes', 'agents:read', 'thread:read'])
    expect(Object.keys(m.reasons).sort()).toEqual([...m.caps].sort())
    // K2's manifest check: each reason is one line of at most 140 characters.
    for (const r of Object.values(m.reasons) as string[]) expect(r.length, r).toBeLessThanOrEqual(140)
    const caps = new Map<string, Any>(CATALOG.caps.map((c: Any) => [c.name, c]))
    for (const c of m.caps) {
      const row = caps.get(c)
      expect(row, c).toBeTruthy()
      expect(row.exposure.some((e: string) => e === 'widget' || e === 'garden-local'), c).toBe(true)
    }
  })

  it('never writes markup, reaches the network or keeps its own storage, and calls only catalog verbs', () => {
    const verbs = new Set<string>(CATALOG.verbs.map((v: Any) => v.verb))
    const runtime = new Set(['connected', 'ready', 'can', 'config', 'onVisible'])
    for (const [name, src] of SCRIPTS) {
      for (const bad of ['innerHTML', 'outerHTML', 'insertAdjacentHTML', 'document.write', 'fetch(', 'XMLHttpRequest', 'WebSocket', 'EventSource', 'localStorage', 'sessionStorage', 'indexedDB', 'eval(', 'new Function']) {
        expect(src.includes(bad), `${name} uses ${bad}`).toBe(false)
      }
      for (const m of src.matchAll(/(?<![\w.])k2\.([a-zA-Z]+(?:\.[a-zA-Z]+)?)/g)) {
        // A verb, a runtime member, or a verb group named in a comment (k2.layout.*).
        const call = m[1]
        const group = [...verbs].some((v) => v.startsWith(call + '.'))
        expect(verbs.has(call) || runtime.has(call.split('.')[0]) || group, `${name} calls k2.${call}`).toBe(true)
      }
    }
  })
})

describe('k2:canvas@1 the canvas', () => {
  it('shows the Garden’s tiles in its order, with its title, and remembers what the person has seen', async () => {
    const m = await started({ notes: true, config: { title: 'Compute', tiles: 'notes,compute,nope' }, nodes: [node('k2n12-vin')] })
    expect(text(m.doc.getElementById('title'))).toBe('Compute')
    expect(m.C.state().offered).toEqual(['notes', 'compute'])
    expect([...m.doc.querySelectorAll('.cv-tile')].map((t: Any) => t.dataset.tile)).toEqual(['notes', 'compute'])
    await until('saved', () => !!m.fake.log.saves.tiles)
    expect(m.fake.log.saves.tiles).toEqual({ v: 1, hidden: [], known: ['notes', 'compute'] })
    expect(m.doc.getElementById('empty').hidden).toBe(true)
  })

  it('hides a tile from its ×, stops it, and shows it again from the Tiles menu', async () => {
    const m = await started({ nodes: [node('k2n12-vin')] })
    await until('the first read', () => m.fake.log.nodes === 1)
    tileEl(m, 'compute').querySelector('.cv-x').click()
    expect(tileEl(m, 'compute')).toBeNull()
    await until('saved hidden', () => (m.fake.log.saves.tiles || {}).hidden?.[0] === 'compute')
    // A hidden tile reads nothing more, even when the frame comes back on screen.
    m.fake.visible(true)
    await sleep(30)
    expect(m.fake.log.nodes).toBe(1)
    m.doc.getElementById('tiles-btn').click()
    expect(m.doc.getElementById('tiles-menu').hidden).toBe(false)
    const item = m.doc.querySelector('.cv-item[data-tile="compute"]')
    expect(item.getAttribute('aria-checked')).toBe('false')
    item.click()
    expect(tileEl(m, 'compute')).not.toBeNull()
    await until('read again', () => m.fake.log.nodes === 2)
    expect(m.fake.log.saves.tiles.hidden).toEqual([])
    expect(m.doc.querySelector('.cv-item[data-tile="compute"]').getAttribute('aria-checked')).toBe('true')
  })

  it('keeps a person’s hidden tiles, and a tile they have never seen starts shown', async () => {
    const m = await started({ notes: true, config: { tiles: 'compute,notes' }, saves: { tiles: { v: 1, hidden: ['compute'], known: ['compute'] } } })
    expect(m.C.state().shown).toEqual(['notes'])
    expect(tileEl(m, 'compute')).toBeNull()
    await until('known saved', () => m.fake.log.saves.tiles.known.length === 2)
    expect(m.fake.log.saves.tiles).toEqual({ v: 1, hidden: ['compute'], known: ['compute', 'notes'] })
  })

  it('makes the Tiles button (and the open menu) holes in K2’s drag strip', async () => {
    const m = await started()
    await until('shared at start', () => m.fake.log.holes.length === 1)
    // JSDOM lays nothing out: every rect is empty, so none is sent, but the list is replaced each time.
    expect(m.fake.log.holes[0]).toEqual([])
    m.doc.getElementById('tiles-btn').click()
    await until('shared when the menu opens', () => m.fake.log.holes.length === 2)
    m.win.dispatchEvent(new m.win.Event('resize'))
    await until('shared on resize', () => m.fake.log.holes.length === 3)
  })

  it('says so when every tile is hidden', async () => {
    const m = await started({ notes: true, config: { tiles: 'compute,notes' }, saves: { tiles: { v: 1, hidden: ['compute', 'notes'], known: ['compute', 'notes'] } } })
    expect(m.doc.querySelectorAll('.cv-tile').length).toBe(0)
    expect(m.doc.getElementById('empty').hidden).toBe(false)
  })

  it('a tile whose verb this K2 doesn’t offer says so, and the others still draw', async () => {
    const m = await started({ notes: true, verbs: ALL_VERBS.filter((v) => v !== 'compute.nodes') })
    const why = tileEl(m, 'compute').querySelector('.cv-tile-body .cv-empty')
    expect(text(why)).toBe('Compute load isn’t available here yet.')
    expect(why.title).toContain('needs compute.nodes')
    expect(text(tileEl(m, 'notes').querySelector('.cv-tile-body'))).toBe('notes')
    expect(m.fake.log.nodes).toBe(0)
  })

  it('works without saves (the preview): in memory only', async () => {
    const m = await started({ notes: true, verbs: ALL_VERBS.filter((v) => !v.startsWith('save.')) })
    tileEl(m, 'notes').querySelector('.cv-x').click()
    expect(m.C.state().hidden).toEqual(['notes'])
    expect(m.win.notesStops).toBe(1)
    expect(m.fake.log.saves).toEqual({})
  })
})

describe('k2:canvas@1 Compute load', () => {
  it('draws each node: running against slots, queued, free CPU and memory, room, held, offline folded', async () => {
    const nodes = [
      node('k2n10-vin', { running: 3, slots: 3, queued: 2, shared: true, servers: 2, queueFrom: 'node' }),
      node('k2n11-vin', { running: 0, slots: 2, cpu: { freeMillis: 8000, totalMillis: 8000, idleMillis: 8000 }, who: [], jobs: [] }),
      node('k2n12-vin', { running: 1, why: 'sew-ci holds a lock' }),
      node('k2n4-vin', { online: false, state: 'offline', running: 0, lastSeenAt: NOW - 7 * 86400 }),
    ]
    const m = await started({ nodes })
    await until('drawn', () => m.doc.querySelectorAll('.cl-node').length === 3)
    const names = [...m.doc.querySelectorAll('.cl-grid:not(.cl-grid-off) .cl-node .cl-name')].map(text)
    expect(names).toEqual(['k2n10-vin', 'k2n12-vin', 'k2n11-vin'])
    const cards = m.doc.querySelectorAll('.cl-node')
    expect(cards[0].classList.contains('cl-full')).toBe(true)
    expect(text(cards[0].querySelector('.cl-count'))).toBe('3/3')
    expect(cards[0].querySelectorAll('.cl-seg-q').length).toBe(2)
    expect(cards[1].classList.contains('cl-blocked')).toBe(true)
    expect(text(cards[1])).toContain('sew-ci holds a lock')
    expect(cards[2].classList.contains('cl-room')).toBe(true)
    expect(text(cards[2])).toContain('room: 2 slots')
    expect(text(m.doc.querySelector('.cl-meta'))).toBe('4/8 running · 2 queued · 1 with room · 3 online')
    expect(text(cards[0])).toContain('20/32 GB free')
    const fold = m.doc.querySelector('.cl-off-head')
    expect(text(fold)).toContain('offline (1): k2n4-vin')
    fold.click()
    expect(text(m.doc.querySelector('.cl-grid-off'))).toContain('seen 7 d ago')
  })

  it('orders by name on A–Z and keeps that choice', async () => {
    const m = await started({ nodes: [node('b', { running: 3 }), node('a', { running: 0 })] })
    await until('drawn', () => m.doc.querySelectorAll('.cl-node').length === 2)
    const btns = m.doc.querySelectorAll('.cl-btn')
    btns[1].click()
    expect([...m.doc.querySelectorAll('.cl-name')].map(text)).toEqual(['a', 'b'])
    await until('saved', () => m.fake.log.saves['tile/compute/sort'] === 'name')
    const again = await started({ nodes: [node('b', { running: 3 }), node('a', { running: 0 })], saves: { 'tile/compute/sort': 'name' } })
    await until('drawn', () => again.doc.querySelectorAll('.cl-node').length === 2)
    expect([...again.doc.querySelectorAll('.cl-name')].map(text)).toEqual(['a', 'b'])
  })

  it('says why when K2 refuses, and draws a node’s text as text', async () => {
    const m = await started({ computeError: true })
    await until('the note', () => !tileEl(m, 'compute').querySelector('.cv-tile-note').hidden)
    expect(text(tileEl(m, 'compute').querySelector('.cv-tile-note'))).toContain('cap not granted')
    const evil = await started({ nodes: [node('<img src=x onerror=alert(1)>', { why: '<b>x</b>' })] })
    await until('drawn', () => evil.doc.querySelectorAll('.cl-node').length === 1)
    expect(evil.doc.querySelector('.cl-node img')).toBeNull()
    expect(evil.doc.querySelector('.cl-node b')).toBeNull()
    expect(text(evil.doc.querySelector('.cl-name'))).toBe('<img src=x onerror=alert(1)>')
  })

  it('a server with no nodes says so', async () => {
    const m = await started({ nodes: [] })
    await until('empty', () => text(tileEl(m, 'compute')).includes('No compute nodes'))
  })
})

describe('k2:canvas@1 Agents', () => {
  const agents = [
    agent('sew', 2, { working: true, activity: 'working' }),
    agent('k2', 0, { image: 'data:image/png;base64,iVBORw0KGgo=' }),
    agent('k2-dev-web', 1, { needsYou: true, activity: 'needs-you', selected: true }),
    agent('far', 3, { address: 'far::nsi.k2.dev', server: 'nsi.k2.dev' }),
    agent('off', 4, { state: 'offline', stateLabel: 'Offline', openable: false }),
  ]
  const unread = { ok: true, total: 5, byConversation: [
    { id: 'c1', addr: 'k2', lastReadSeq: 1, lastSeq: 4, unread: 3 },
    { id: 'c2', addr: 'sew', lastReadSeq: 1, lastSeq: 2, unread: 1, waiting: 1 },
    { id: 'c3', addr: 'far', lastReadSeq: 0, lastSeq: 9, unread: 9 },
    { id: 'c4', addr: 'k2-dev-web', lastReadSeq: 0, lastSeq: 2, unread: 2, muted: true },
  ] }

  it('lists every agent in K2’s order with its picture or initials, its state and your unread', async () => {
    const m = await started({ agents, unread })
    await until('listed', () => m.doc.querySelectorAll('.ag-row').length === 5 && m.fake.log.unread > 0 && !!m.doc.querySelector('.ag-badge'))
    const rows = [...m.doc.querySelectorAll('.ag-row')]
    expect(rows.map((r: Any) => text(r.querySelector('.ag-name')))).toEqual(['k2', 'k2-dev-web', 'sew', 'far', 'off'])
    expect(rows[0].querySelector('img').getAttribute('src')).toBe('data:image/png;base64,iVBORw0KGgo=')
    expect(text(rows[2].querySelector('.ag-face'))).toBe('S')
    expect(text(rows[0].querySelector('.ag-badge'))).toBe('3')
    expect(text(rows[2].querySelector('.ag-badge'))).toBe('2')
    expect(rows[1].querySelector('.ag-badge')).toBeNull()
    expect(rows[3].querySelector('.ag-badge')).toBeNull()
    expect(text(rows[1].querySelector('.ag-sub'))).toBe('needs you')
    expect(text(rows[2].querySelector('.ag-sub'))).toBe('working')
    expect(text(rows[3].querySelector('.ag-sub'))).toBe('nsi.k2.dev')
    expect(rows[1].classList.contains('ag-sel')).toBe(true)
    expect(rows[4].disabled).toBe(true)
    expect(text(m.doc.querySelector('[data-tile="agents"] .cv-tile-meta'))).toBe('5 agents · 1 working · 1 needs you · 5 unread')
  })

  it('a pick opens that agent in the Thread, unfolding a folded Thread column', async () => {
    const layout = { columns: [{ index: 0, label: null, collapsible: false, collapsed: false }, { index: 1, label: 'Thread', collapsible: true, collapsed: true }] }
    const m = await started({ agents, unread, layout })
    await until('listed', () => m.doc.querySelectorAll('.ag-row').length === 5)
    await sleep(10)
    m.doc.querySelectorAll('.ag-row')[3].click()
    await until('opened', () => m.fake.log.opened.length === 1)
    expect(m.fake.log.opened).toEqual(['far::nsi.k2.dev'])
    expect(m.fake.log.layoutSet).toEqual([{ column: 1, collapsed: false }])
  })

  it('finds an agent by name once the list is long, and stops its subscription when hidden', async () => {
    const many = Array.from({ length: 10 }, (_, i) => agent('agent-' + i, i))
    const m = await started({ agents: many })
    await until('listed', () => m.doc.querySelectorAll('.ag-row').length === 10)
    const search = m.doc.querySelector('.ag-search')
    expect(search.hidden).toBe(false)
    search.value = 'agent-7'
    search.dispatchEvent(new m.win.Event('input'))
    expect([...m.doc.querySelectorAll('.ag-name')].map(text)).toEqual(['agent-7'])
    tileEl(m, 'agents').querySelector('.cv-x').click()
    expect(m.fake.log.agentsOff).toBe(1)
  })
})
