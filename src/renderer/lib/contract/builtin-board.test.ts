// The built-in Board widget, k2:board@1 (the Board Garden): its readable files, loaded in index.html's
// order into a fresh JSDOM per test, against the shared stand-in `k2` in tests/board/fake-k2.js (K2's
// Garden data contract: revs, ifRev conflicts, subscribe = whole state then each change, sha256 blobs).
// What it pins:
//   - store only: a fresh box makes its first board once, even with two windows racing;
//   - add, edit, delete (with the cascade: lines, mind-map children, pinned comments), undo;
//   - an agent's change shows live with its handle; a conflict re-reads and re-applies only our fields;
//   - people by key (person:owner, agent:<handle>): reactions and votes per person, older shapes read;
//   - ids on checklist items, replies, table rows and columns (older lists get ids by position);
//   - typed refusals (too long, too many): said in a toast, nothing cut, nothing written;
//   - pictures as blob refs from the original bytes, never a blank or data: image; older data: images show;
//   - a line whose attached end is gone is never drawn to (0, 0);
//   - the JSON export: sorted objects, person tokens, {blob} refs with the bytes, no K2 identifiers;
//   - the For agents panel's grant command; the Chat column's agents from K2's grants (with a fallback);
//   - the Garden-only parts behind BoardHost.features; and the shipped files' hygiene.
// Browser-level checks (real pointer and keys, WebKit, PNG export, a 1,000-object board) are the
// Playwright specs in tests/board/.
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'
import { webcrypto } from 'node:crypto'
import { describe, expect, it } from 'vitest'

const { JSDOM } = createRequire(import.meta.url)('jsdom') as {
  JSDOM: new (html: string, opts: { runScripts: 'outside-only'; pretendToBeVisual: boolean }) => { window: unknown }
}

const ROOT = resolve(__dirname, '../../../..')
const DIR = resolve(ROOT, 'crates/k2-core/src/zen/builtin_widgets/board-1')
const read = (f: string): string => readFileSync(resolve(DIR, f), 'utf8')
const HTML = read('index.html')
const ORDER = [...HTML.matchAll(/<script src="([^"]+)"><\/script>/g)].map((m) => m[1])
const SCRIPTS = ORDER.map((f) => [f, read(f)] as const)
const FAKE = readFileSync(resolve(ROOT, 'tests/board/fake-k2.js'), 'utf8')
const PNG_1x1 = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64')

/* eslint-disable @typescript-eslint/no-explicit-any */
type Any = any

// What JSDOM lacks and Board uses: matchMedia, a 2d canvas, hit testing, SubtleCrypto, image decoding.
function shim(win: Any): void {
  win.matchMedia = () => ({ matches: false, addEventListener() {}, removeEventListener() {} })
  const ctx = new Proxy({}, { get: (_t, k) => (k === 'measureText' ? (s: string) => ({ width: String(s).length * 7 }) : k === 'getImageData' ? () => ({ data: new Uint8ClampedArray(4) }) : () => {}), set: () => true })
  win.HTMLCanvasElement.prototype.getContext = () => ctx
  win.document.elementFromPoint = () => null
  win.document.elementsFromPoint = () => []
  win.TextEncoder = TextEncoder
  win.TextDecoder = TextDecoder
  if (!win.crypto.subtle) Object.defineProperty(win.crypto, 'subtle', { value: webcrypto.subtle })
  // Decodes only a PNG (signature + IHDR), like a browser that can't read anything else in these bytes.
  win.createImageBitmap = async (blob: Any) => {
    const bytes: Uint8Array = await new Promise((res, rej) => { const r = new win.FileReader(); r.onload = () => res(new Uint8Array(r.result)); r.onerror = () => rej(r.error); r.readAsArrayBuffer(blob) })
    const png = bytes.length > 24 && bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47
    if (!png) throw new Error('InvalidStateError: the source image could not be decoded')
    const dv = new DataView(bytes.buffer, bytes.byteOffset)
    return { width: dv.getUint32(16), height: dv.getUint32(20), close() {} }
  }
}

interface Mounted { win: Any; doc: Any; fake: Any; App: Any; S: Any }
function mount(opts: Any = {}, shared?: Any): Mounted {
  const dom = new JSDOM(HTML.replace(/<script[^>]*><\/script>/g, '').replace(/<link[^>]*>/g, ''), { runScripts: 'outside-only', pretendToBeVisual: true })
  const win = dom.window as Any
  shim(win)
  win.eval(FAKE)
  const fake = shared || win.makeFakeK2(opts)
  win.k2 = fake.k2
  for (const [name, src] of SCRIPTS) {
    win.eval(src)
    if (name === 'host.js' && opts.features) win.BoardHost.features = opts.features
  }
  return { win, doc: win.document, fake, App: win.BoardApp, S: win.BoardSchema }
}
const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms))
async function until(what: string, ok: () => boolean, ms = 5000): Promise<void> {
  const t0 = Date.now()
  while (Date.now() - t0 < ms) { if (ok()) return; await sleep(5) }
  throw new Error('timed out waiting for: ' + what)
}
async function started(opts: Any = {}): Promise<Mounted> {
  const m = mount(opts)
  await until('the board opens', () => m.fake.log.ready === 1 && !!m.App.store.boardId)
  return m
}
const statusOf = (m: Mounted): string => m.doc.querySelector('.status').textContent
const toast = (m: Mounted): string => m.doc.getElementById('toast').textContent
const stored = (m: Mounted, key: string): Any => { const e = m.fake.space.get(key); return e ? e.value : undefined }
const boardKeys = (m: Mounted): string[] => [...m.fake.space.keys()].filter((k: string) => k.startsWith('boards/'))
// Save now and wait until nothing for the open board is waiting.
async function saved(m: Mounted): Promise<void> {
  await until('saved', () => { m.App.store.flushNow(); return statusOf(m) === 'Saved' })
}
const P = 'board/b0a0a0a/'
const BOARD = (extra: Array<[string, unknown]> = []): Array<[string, unknown]> => [
  ['seeded', { at: '2026-01-01T00:00:00Z' }],
  ['boards/b0a0a0a', { id: 'b0a0a0a', name: 'Plans', project: null, format: 1 }],
  [P + 'meta', { format: 1, title: 'Plans' }],
  ...extra,
]
const sticky = (id: string, f: Any = {}): [string, Any] => [P + 'obj/' + id, Object.assign({ id, type: 'sticky', x: 0, y: 0, w: 200, h: 200, z: 1, text: id, color: 'yellow', createdBy: 'agent:helper', createdAt: '2026-01-01T00:00:00Z', updatedBy: 'agent:helper', updatedAt: '2026-01-01T00:00:00Z' }, f)]

describe('k2:board@1 shipped files', () => {
  const FILES = ['manifest.json', 'index.html', 'board.css', ...ORDER]
  const JS = ORDER
  const ADAPTERS = ['host.js', 'store.js']

  it('loads its scripts in build order, ships nothing else, and stays under 512 KiB', () => {
    expect(ORDER).toEqual(['schema.js', 'host.js', 'store.js', 'model.js', 'geom.js', 'icons.js', 'render.js', 'app.js', 'export.js', 'ui.js'])
    const shipped = readFileSync(resolve(DIR, 'index.html'), 'utf8')
    expect(shipped).not.toMatch(/board-data\.js|\.min\.js/)
    const code = ['index.html', 'board.css', ...ORDER].reduce((n, f) => n + Buffer.byteLength(read(f)), 0)
    expect(code).toBeLessThan(512 * 1024)
  })

  it('declares exactly its caps, each with a short plain reason, and K2’s PNG encoder', () => {
    const m = JSON.parse(read('manifest.json'))
    expect(m.caps).toEqual(['store:read', 'store:write', 'thread:post', 'agents:read', 'links:open', 'downloads:save', 'uploads:pick'])
    expect(Object.keys(m.reasons).sort()).toEqual([...m.caps].sort())
    for (const r of Object.values(m.reasons) as string[]) {
      expect(r.length).toBeLessThanOrEqual(140)
      expect(r).not.toMatch(/k2\.|store\.|grant|keeper|Thread path/i)
    }
    expect(m.requires).toEqual({ libs: ['k2-png@1'] })
  })

  it('names nobody: no people, agents, paths, demo companies or hosts from where it was built', () => {
    for (const f of FILES) {
      const s = read(f)
      for (const bad of [/baden/i, /appa/i, /board-builder/i, /keeper/i, /rosson/i, /\/Users\//, /switchback/i, /[a-z0-9-]+\.k2\.dev/i, /OWNER\s*=/]) {
        expect(s, f + ' has ' + bad).not.toMatch(bad)
      }
    }
  })

  it('is sealed-frame clean: no HTML strings, network, storage, eval or modules; inline-free index.html', () => {
    for (const f of JS) {
      const s = read(f)
      for (const bad of ['innerHTML', 'outerHTML', 'insertAdjacentHTML', 'document.write', 'fetch(', 'XMLHttpRequest', 'WebSocket', 'EventSource', 'localStorage', 'sessionStorage', 'indexedDB', 'eval(', 'new Function', 'import ']) {
        expect(s.includes(bad), f + ' uses ' + bad).toBe(false)
      }
    }
    const html = read('index.html')
    for (const bad of [' onclick=', ' onload=', ' onerror=', ' on', '<iframe', '<object', '<embed', '<base', 'http-equiv', 'src="http', 'href="http', '//cdn']) {
      expect(html.includes(bad === ' on' ? ' onkeydown=' : bad), 'index.html has ' + bad).toBe(false)
    }
    expect(html).not.toMatch(/\son[a-z]+=/)
  })

  it('only host.js and store.js call k2, and only widget verbs, runtime members or SDK helpers', () => {
    for (const f of JS.filter((x) => !ADAPTERS.includes(x))) expect(read(f).includes('k2.'), f + ' mentions k2.').toBe(false)
    const catalog = JSON.parse(readFileSync(resolve(ROOT, 'crates/k2-core/src/contract/catalog.json'), 'utf8'))
    const verbs = new Set<string>(catalog.verbs.filter((v: Any) => (v.exposure || []).some((e: string) => e === 'widget' || e === 'garden-local')).map((v: Any) => v.verb))
    verbs.add('store.grantees')   // the garden-local verb the Board Garden adds (K2 side, same release)
    const RUNTIME = ['call', 'subscribe', 'on', 'connected', 'can', 'ready', 'config', 'widget', 'asset', 'motion', 'open', 'download', 'pick', 'onDrop']
    const SDK = ['store.subscribe', 'store.tx', 'viewer', 'blobs.url', 'save.get', 'save.set', 'save.remove', 'save.keys']
    const seen: string[] = []
    for (const f of ADAPTERS) {
      const js = read(f)
      for (const mm of js.matchAll(/k2\.([A-Za-z0-9.]+)/g)) {
        const call = mm[1].replace(/\.$/, '')
        const head = call.split('.')[0]
        const ok = verbs.has(call) || RUNTIME.includes(call) || SDK.includes(call) || (['motion', 'connected', 'widget', 'config'].includes(head) && RUNTIME.includes(head))
        expect(ok, f + ' calls k2.' + call).toBe(true)
        seen.push(call)
      }
    }
    // The verbs the Garden relies on (K2 must answer each).
    for (const v of ['store.batch', 'store.subscribe', 'store.get', 'store.list', 'store.changes', 'store.grantees', 'blobs.put', 'blobs.get', 'blobs.url', 'save.get', 'save.set', 'viewer', 'agents.list', 'agents.show', 'conversation.setContext', 'conversation.open', 'compose.draft', 'layout.get', 'layout.set', 'layout.toggle', 'layout.subscribe', 'download', 'pick', 'onDrop', 'open', 'theme.get', 'theme.changed', 'canvas.setHitRegions']) {
      expect(seen, 'k2.' + v).toContain(v)
    }
    // The old save path is gone.
    for (const f of JS) for (const bad of ['thread.post', 'thread.read', 'thread.unread', 'board1 ', 'hasK2Data', 'BOARD_DATA', 'accessOf', 'setAccess', 'rowCopy', 'settings/access', "'acl/"]) expect(read(f).includes(bad), f + ' has ' + bad).toBe(false)
  })
})

describe('k2:board@1 schema (data format 1)', () => {
  const S = mount().S
  const code = (f: () => unknown): Any => { try { f(); return 'ok' } catch (e: Any) { expect(e.name).toBe('BoardRefusal'); return { code: e.code, field: e.field, max: e.max } } }

  it('refuses with a typed error instead of cutting or skipping', () => {
    expect(code(() => S.clean('sticky', { text: 'x'.repeat(5001) }))).toEqual({ code: 'too_long', field: 'text', max: 5000 })
    expect(S.clean('sticky', { text: 'x'.repeat(5000) }).text).toHaveLength(5000)
    expect(code(() => S.clean('sticky', { tags: Array.from({ length: 201 }, (_, i) => 't' + i) }))).toEqual({ code: 'too_many', field: 'tags', max: 200 })
    expect(code(() => S.clean('pen', { points: Array.from({ length: 5001 }, () => [0, 0]) }))).toEqual({ code: 'too_many', field: 'points', max: 5000 })
    expect(code(() => S.clean('table', { rows: Array.from({ length: 101 }, () => ['']) }))).toEqual({ code: 'too_many', field: 'rows', max: 100 })
    expect(code(() => S.clean('table', { rows: [Array.from({ length: 31 }, () => '')] }))).toEqual({ code: 'too_many', field: 'rows', max: 30 })
    expect(code(() => S.clean('card', { checks: Array.from({ length: 51 }, () => ({ text: '' })) }))).toEqual({ code: 'too_many', field: 'checks', max: 50 })
    expect(code(() => S.clean('sticky', { color: 'plaid' }))).toEqual({ code: 'bad_field', field: 'color', max: undefined })
    expect(code(() => S.clean('sticky', { nope: 1, Nope: 2 }))).toEqual({ code: 'unknown_field', field: 'nope', max: undefined })
    expect(code(() => S.clean('blob', { x: 1 }))).toEqual({ code: 'unknown_type', field: 'type', max: undefined })
    expect(code(() => S.checkSize({ text: 'x'.repeat(61 * 1024) }))).toEqual({ code: 'value_too_large', field: null, max: 61440 })
    // Characters are counted as code points (an emoji is one).
    expect(code(() => S.clean('mind', { text: '😀'.repeat(1000) }))).toBe('ok')
  })

  it('keeps images as blob refs; an older data: image is read, a blank or broken one is refused', () => {
    const ref = 'k2blob:sha256-' + 'a'.repeat(64)
    expect(S.clean('image', { src: ref }).src).toBe(ref)
    expect(S.clean('image', { src: 'data:image/png;base64,' + PNG_1x1.toString('base64') }).src).toMatch(/^data:image\/png;base64,/)
    expect(code(() => S.clean('image', { src: 'data:,' }))).toEqual({ code: 'bad_field', field: 'src', max: undefined })
    expect(code(() => S.clean('image', { src: 'https://example.com/a.png' }))).toEqual({ code: 'bad_field', field: 'src', max: undefined })
    expect(code(() => S.clean('image', { src: 'k2blob:sha256-xyz' }))).toEqual({ code: 'bad_field', field: 'src', max: undefined })
  })

  it('reads reactions per person (turning the older per-emoji shape round) and votes per person', () => {
    expect(S.clean('sticky', { reactions: { '👍': ['Ada', 'agent:helper'], '🎉': ['Ada'] } }).reactions).toEqual({ Ada: ['👍', '🎉'], 'agent:helper': ['👍'] })
    expect(S.clean('sticky', { reactions: { 'person:owner': ['👍', '👍', '🔥'], 'agent:x': [] } }).reactions).toEqual({ 'person:owner': ['👍', '🔥'] })
    expect(code(() => S.clean('sticky', { reactions: { 'person:owner': ['🍕'] } }))).toEqual({ code: 'bad_field', field: 'reactions', max: undefined })
    expect(S.clean('sticky', { votes: { 'person:owner': 2, 'agent:x': 0 } }).votes).toEqual({ 'person:owner': 2 })
    expect(code(() => S.clean('sticky', { votes: { 'person:owner': 1.5 } }))).toEqual({ code: 'bad_field', field: 'votes', max: undefined })
  })

  it('gives list items ids; older items without one get them by position', () => {
    expect(S.clean('card', { checks: [{ text: 'a', done: true }, 'b'] }).checks).toEqual([{ id: 'ck-0', text: 'a', done: true }, { id: 'ck-1', text: 'b', done: false }])
    expect(S.clean('comment', { replies: [{ by: 'Ada', at: 't', text: 'hi' }] }).replies).toEqual([{ id: 'rp-0', by: 'Ada', at: 't', text: 'hi' }])
    expect(S.ensureIds({ type: 'table', rows: [['a', 'b'], ['c']] })).toMatchObject({ rowIds: ['rw-0', 'rw-1'], colIds: ['cl-0', 'cl-1'] })
    expect(code(() => S.clean('table', { rows: [['a'], ['b']], rowIds: ['rw-1'] }))).toEqual({ code: 'bad_field', field: 'rowIds', max: undefined })
    expect(S.itemId('ck')).toMatch(/^ck-[0-9a-f]{8}$/)
  })
})

describe('k2:board@1 in the Garden', () => {
  it('a fresh box makes one first board, once, even with two windows starting together', async () => {
    const a = mount({ raceSeed: true })
    const b = mount({}, a.fake)
    await until('both open', () => a.fake.log.ready === 2 && !!a.App.store.boardId && !!b.App.store.boardId)
    expect(boardKeys(a)).toHaveLength(1)
    const id = boardKeys(a)[0].slice('boards/'.length)
    expect(id).toMatch(/^b[0-9a-f]{6}$/)
    expect(a.App.store.boardId).toBe(id)
    expect(b.App.store.boardId).toBe(id)
    expect(stored(a, 'boards/' + id)).toEqual({ id, name: 'Board', project: null, format: 1 })
    expect(stored(a, 'board/' + id + '/meta')).toEqual({ format: 1, title: 'Board' })
    expect(stored(a, 'seeded')).toMatchObject({ at: expect.any(String) })
    // Both windows tried, one in a single batch with ifRev 0 everywhere, and only one board landed.
    const seeds = a.fake.log.batches.filter((ops: Any[]) => ops.some((o) => o.key === 'seeded'))
    expect(seeds).toHaveLength(2)
    for (const ops of seeds) {
      expect(ops.map((o: Any) => o.key)).toEqual(['seeded', expect.stringMatching(/^boards\/b[0-9a-f]{6}$/), expect.stringMatching(/^board\/b[0-9a-f]{6}\/meta$/)])
      expect(ops.every((o: Any) => o.op === 'put' && o.ifRev === 0)).toBe(true)
    }
    expect(statusOf(a)).toBe('Saved')
    expect(statusOf(b)).toBe('Saved')
  })

  it('adds, edits and deletes objects as one key each, stamped with person:owner', async () => {
    const m = await started({ seed: BOARD() })
    expect(m.App.store.boardId).toBe('b0a0a0a')
    const id = m.App.create('sticky', { x: 10, y: 20, text: 'Hello' })
    await saved(m)
    const v = stored(m, P + 'obj/' + id)
    expect(v).toMatchObject({ id, type: 'sticky', x: 10, y: 20, text: 'Hello', createdBy: 'person:owner', updatedBy: 'person:owner', batch: expect.stringMatching(/^w-/) })
    m.App.model.apply([{ op: 'set', id, f: { text: 'Hi there' } }])
    await saved(m)
    expect(stored(m, P + 'obj/' + id)).toMatchObject({ text: 'Hi there', x: 10, createdBy: 'person:owner' })
    m.App.select([id]); m.App.del()
    await saved(m)
    expect(stored(m, P + 'obj/' + id)).toBeUndefined()
    // Every write was an ifRev write.
    for (const ops of m.fake.log.batches) for (const o of ops) expect(typeof o.ifRev).toBe('number')
  })

  it('deleting an object takes its lines, mind-map children and pinned comments with it; undo brings them back', async () => {
    const m = await started({ seed: BOARD([
      sticky('h-0000000001', { type: 'shape', shape: 'round', fill: 'white' }),
      sticky('s-0000000002'),
      [P + 'obj/c-0000000003', { id: 'c-0000000003', type: 'connector', from: { id: 'h-0000000001', anchor: 'auto' }, to: { id: 's-0000000002', anchor: 'auto' }, createdBy: 'person:owner' }],
      [P + 'obj/m-0000000004', { id: 'm-0000000004', type: 'mind', text: 'Root', x: 500, y: 0, w: 180, h: 48 }],
      [P + 'obj/m-0000000005', { id: 'm-0000000005', type: 'mind', text: 'Kid', parent: 'm-0000000004', x: 700, y: 0, w: 160, h: 48 }],
      [P + 'obj/m-0000000006', { id: 'm-0000000006', type: 'mind', text: 'Grandkid', parent: 'm-0000000005', x: 900, y: 0, w: 160, h: 48 }],
      [P + 'obj/q-0000000007', { id: 'q-0000000007', type: 'comment', text: 'Why?', target: 'h-0000000001', x: 0, y: 0 }],
    ]) })
    await until('objects shown', () => !!m.App.model.get('q-0000000007'))
    m.App.select(['h-0000000001']); m.App.del()
    m.App.select(['m-0000000004']); m.App.del()
    await saved(m)
    const left = [...m.fake.space.keys()].filter((k: string) => k.startsWith(P + 'obj/')).sort()
    expect(left).toEqual([P + 'obj/s-0000000002'])
    m.App.model.undo(); m.App.model.undo()
    await saved(m)
    expect([...m.fake.space.keys()].filter((k: string) => k.startsWith(P + 'obj/')).sort()).toHaveLength(7)
    expect(stored(m, P + 'obj/c-0000000003')).toMatchObject({ from: { id: 'h-0000000001' }, createdBy: 'person:owner' })
    expect(stored(m, P + 'obj/s-0000000002')).toMatchObject({ createdBy: 'agent:helper' })
  })

  it('an agent’s change shows at once, under its handle and K2’s name for it', async () => {
    const m = await started({ seed: BOARD() })
    m.fake.agentWrite([{ key: P + 'obj/s-00000000aa', value: { id: 's-00000000aa', type: 'sticky', x: 5, y: 5, text: 'From an agent', updatedBy: 'agent:helper' } }], 'helper')
    await until('the agent’s sticky', () => !!m.App.model.get('s-00000000aa'))
    const o = m.App.model.get('s-00000000aa')
    expect(o.updatedBy).toBe('agent:helper')
    expect(m.App.model.nameOf(o.updatedBy)).toBe('Helper')
    expect(m.App.model.nameOf('person:owner')).toBe('Ada')
    expect(m.doc.querySelector('[data-id="s-00000000aa"] .txt').textContent).toBe('From an agent')
    // A change K2 marks as the person's (another window) is the person's.
    m.fake.otherWindow([{ key: P + 'obj/s-00000000aa', value: Object.assign({}, stored(m, P + 'obj/s-00000000aa'), { text: 'Edited in another window' }) }])
    await until('the other window’s edit', () => m.App.model.get('s-00000000aa').text === 'Edited in another window')
    expect(m.App.model.isMe(m.App.model.get('s-00000000aa').updatedBy)).toBe(true)
  })

  it('on a conflict, reads the newer value and re-applies only the fields changed here', async () => {
    const m = await started({ seed: BOARD([sticky('s-0000000001', { extraField: { keep: true } })]) })
    await until('shown', () => !!m.App.model.get('s-0000000001'))
    m.fake.sneak(P + 'obj/s-0000000001', Object.assign({}, stored(m, P + 'obj/s-0000000001'), { x: 999, color: 'pink' }))
    m.App.model.apply([{ op: 'set', id: 's-0000000001', f: { text: 'Mine' } }])
    await saved(m)
    expect(stored(m, P + 'obj/s-0000000001')).toMatchObject({ x: 999, color: 'pink', text: 'Mine', extraField: { keep: true } })
    expect(m.App.model.get('s-0000000001')).toMatchObject({ x: 999, color: 'pink', text: 'Mine' })
  })

  it('reactions and votes are kept per person; an older entry under the viewer’s name is theirs', async () => {
    const m = await started({ viewer: 'Ada', seed: BOARD([sticky('s-0000000001', { reactions: { '👍': ['ada', 'Bob'] }, votes: { Ada: 1 } })]) })
    await until('shown', () => !!m.App.model.get('s-0000000001'))
    expect(m.App.model.get('s-0000000001').reactions).toEqual({ ada: ['👍'], Bob: ['👍'] })
    // (JSDOM's selector engine can't match an emoji attribute value: find the chip by its data.)
    const chip = (): Any => [...m.doc.querySelectorAll('[data-id="s-0000000001"] .react')].find((c: Any) => c.dataset.emoji === '👍')
    expect(chip().classList.contains('mine')).toBe(true)
    expect(chip().title).toBe('Ada, Bob')   // the viewer's older name reads as the viewer
    m.App.toggleReaction('s-0000000001', '👍')   // takes the person's off, wherever it is
    m.App.toggleReaction('s-0000000001', '🎉')
    await saved(m)
    expect(stored(m, P + 'obj/s-0000000001').reactions).toEqual({ Bob: ['👍'], 'person:owner': ['🎉'] })
    // Votes: person:owner from now on; the older 'Ada' vote counts against the same allowance.
    m.App.model.apply([{ op: 'meta', f: { voting: { active: true, perPerson: 2 } } }])
    m.App.fire('vote', 's-0000000001', 1)
    m.App.fire('vote', 's-0000000001', 1)
    expect(toast(m)).toBe('No votes left')
    await saved(m)
    expect(stored(m, P + 'obj/s-0000000001').votes).toEqual({ Ada: 1, 'person:owner': 1 })
  })

  it('checklist items, replies and table rows keep ids; older ones get ids by position, stored at the next edit', async () => {
    const m = await started({ seed: BOARD([
      [P + 'obj/k-0000000001', { id: 'k-0000000001', type: 'card', title: 'T', checks: [{ text: 'one', done: false }, { text: 'two', done: true }] }],
      [P + 'obj/q-0000000002', { id: 'q-0000000002', type: 'comment', text: 'Hm', replies: [{ by: 'Ada', at: '2026-01-01T00:00:00Z', text: 'Old reply' }] }],
      [P + 'obj/b-0000000003', { id: 'b-0000000003', type: 'table', rows: [['a', 'b'], ['c', 'd']] }],
    ]) })
    await until('shown', () => !!m.App.model.get('b-0000000003'))
    expect(m.App.model.get('k-0000000001').checks.map((c: Any) => c.id)).toEqual(['ck-0', 'ck-1'])
    expect(m.App.model.get('q-0000000002').replies[0].id).toBe('rp-0')
    expect(m.App.model.get('b-0000000003')).toMatchObject({ rowIds: ['rw-0', 'rw-1'], colIds: ['cl-0', 'cl-1'] })
    // Read only: nothing written back yet.
    expect(stored(m, P + 'obj/k-0000000001').checks[0].id).toBeUndefined()
    expect(m.fake.log.batches.filter((ops: Any[]) => ops.some((o) => o.key.includes('/obj/')))).toHaveLength(0)
    // The next edit of each object stores the ids.
    m.App.model.apply([{ op: 'set', id: 'k-0000000001', f: { x: 40 } }, { op: 'set', id: 'q-0000000002', f: { resolved: true } }, { op: 'set', id: 'b-0000000003', f: { y: 10 } }])
    await saved(m)
    expect(stored(m, P + 'obj/k-0000000001').checks).toEqual([{ id: 'ck-0', text: 'one', done: false }, { id: 'ck-1', text: 'two', done: true }])
    expect(stored(m, P + 'obj/q-0000000002').replies[0]).toEqual({ id: 'rp-0', by: 'Ada', at: '2026-01-01T00:00:00Z', text: 'Old reply' })
    expect(stored(m, P + 'obj/b-0000000003')).toMatchObject({ rowIds: ['rw-0', 'rw-1'], colIds: ['cl-0', 'cl-1'] })
    // A tick keeps the item's id; a new table mints random ids.
    const card = m.App.model.get('k-0000000001')
    m.App.model.apply([{ op: 'set', id: card.id, f: { checks: card.checks.map((c: Any) => (c.id === 'ck-0' ? Object.assign({}, c, { done: true }) : c)) } }])
    const t = m.App.create('table', { x: 0, y: 400 })
    await saved(m)
    expect(stored(m, P + 'obj/k-0000000001').checks[0]).toEqual({ id: 'ck-0', text: 'one', done: true })
    const tv = stored(m, P + 'obj/' + t)
    expect(tv.rowIds).toHaveLength(3); expect(tv.colIds).toHaveLength(3)
    for (const x of tv.rowIds) expect(x).toMatch(/^rw-[0-9a-f]{8}$/)
    for (const x of tv.colIds) expect(x).toMatch(/^cl-[0-9a-f]{8}$/)
    // A person's row change without its ids is refused, not guessed.
    expect(m.App.model.apply([{ op: 'set', id: t, f: { rows: [['x']] } }])).toBe(false)
    expect(toast(m)).toBe('That can’t be saved: “rowIds” isn’t valid')
  })

  it('a refusal is said and nothing is cut or saved', async () => {
    const m = await started({ seed: BOARD([sticky('s-0000000001', { text: 'Short' })]) })
    await until('shown', () => !!m.App.model.get('s-0000000001'))
    const before = m.fake.log.batches.length
    expect(m.App.model.apply([{ op: 'set', id: 's-0000000001', f: { text: 'y'.repeat(5001), x: 77 } }])).toBe(false)
    expect(toast(m)).toBe('That’s too long to save: at most 5,000 characters')
    expect(m.App.model.get('s-0000000001')).toMatchObject({ text: 'Short', x: 0 })
    expect(m.App.model.apply([{ op: 'set', id: 's-0000000001', f: { tags: Array.from({ length: 201 }, (_, i) => 'tag' + i) } }])).toBe(false)
    expect(toast(m)).toBe('That’s too many to save: at most 200 tags')
    expect(m.App.model.apply([{ op: 'set', id: 's-0000000001', f: { extraBig: 'z'.repeat(70000) } }])).toBe(false)
    expect(toast(m)).toBe('That’s too big to save: one item holds at most 60 KB')
    // A whole multi-op edit is refused together.
    const n = Object.keys(m.App.model.objects).length
    expect(m.App.model.apply([{ op: 'add', obj: { id: 's-00000000b1', type: 'sticky', text: 'ok' } }, { op: 'add', obj: { id: 's-00000000b2', type: 'sticky', text: 'x'.repeat(6000) } }])).toBe(false)
    expect(Object.keys(m.App.model.objects)).toHaveLength(n)
    await sleep(500); m.App.store.flushNow(); await sleep(50)
    expect(m.fake.log.batches.length).toBe(before)
    expect(stored(m, P + 'obj/s-0000000001').text).toBe('Short')
    // Typing too much into a sticky: the edit isn't saved, and the original text stays.
    m.App.startEdit('s-0000000001')
    const ta = m.doc.querySelector('textarea.editor'); ta.value = 'q'.repeat(5001)
    m.App.endEdit(true)
    expect(m.App.model.get('s-0000000001').text).toBe('Short')
    expect(toast(m)).toBe('That’s too long to save: at most 5,000 characters')
  })

  it('a pasted picture is stored as the original file, by ref; empty or broken bytes are refused', async () => {
    const m = await started({ seed: BOARD(), urlOf: (ref: string, px: number) => 'blob:pictures/' + ref.slice(-6) + '/' + px })
    const file = new m.win.File([new Uint8Array(PNG_1x1)], 'Team photo.png', { type: 'image/png' })
    const id = await m.App.addImageFile(file)
    expect(id).toMatch(/^i-[0-9a-f]{10}$/)
    expect(m.fake.log.blobPuts).toHaveLength(1)
    const ref = m.fake.log.blobPuts[0].ref
    expect(ref).toMatch(/^k2blob:sha256-[0-9a-f]{64}$/)
    expect(Buffer.from(m.fake.blobs.get(ref).bytes)).toEqual(PNG_1x1)   // the original bytes, not a re-encode
    expect(m.fake.blobs.get(ref).name).toBe('Team photo.png')
    expect(m.App.model.get(id)).toMatchObject({ type: 'image', src: ref, alt: 'Team photo', w: 1, h: 1 })   // the picture's own shape
    await until('the picture shows', () => m.doc.querySelector('[data-id="' + id + '"] img').getAttribute('src') === 'blob:pictures/' + ref.slice(-6) + '/256')
    await saved(m)
    expect(stored(m, P + 'obj/' + id).src).toBe(ref)
    // Nothing that doesn't decode becomes an image.
    const n = Object.keys(m.App.model.objects).length
    expect(await m.App.addImageFile(new m.win.File([new Uint8Array(0)], 'empty.png', { type: 'image/png' }))).toBeNull()
    expect(toast(m)).toBe('That file isn’t a picture Board can show: nothing was added')
    expect(await m.App.addImageFile(new m.win.File([new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10])], 'notes.png', { type: 'image/png' }))).toBeNull()
    expect(Object.keys(m.App.model.objects)).toHaveLength(n)
    expect(m.fake.log.blobPuts).toHaveLength(1)
    // And no image is ever made from a data:, URL.
    expect(m.App.model.apply([{ op: 'add', obj: { id: 'i-00000000ff', type: 'image', src: 'data:,' } }])).toBe(false)
  })

  it('picked and dropped pictures are checked before they are placed', async () => {
    const m = await started({ seed: BOARD() })
    const good = await m.fake.putBytes(new m.win.Uint8Array(PNG_1x1), 'pick.png')
    const bad = await m.fake.putBytes(new m.win.Uint8Array([9, 9, 9, 9]), 'broken.png')
    m.fake.drop([{ ref: good.ref, name: 'pick.png', mime: 'image/png', size: PNG_1x1.length }, { ref: bad.ref, name: 'broken.png', mime: 'image/png', size: 4 }], 300, 200)
    await until('the dropped picture', () => m.App.model.all().some((o: Any) => o.type === 'image'))
    await sleep(50)
    const imgs = m.App.model.all().filter((o: Any) => o.type === 'image')
    expect(imgs).toHaveLength(1)
    expect(imgs[0].src).toBe(good.ref)
    expect(toast(m)).toBe('That file isn’t a picture Board can show: nothing was added')
  })

  it('an older data: picture still shows as it is', async () => {
    const src = 'data:image/png;base64,' + PNG_1x1.toString('base64')
    const m = await started({ seed: BOARD([[P + 'obj/i-0000000001', { id: 'i-0000000001', type: 'image', x: 0, y: 0, w: 40, h: 40, src }]]) })
    await until('shown', () => !!m.doc.querySelector('[data-id="i-0000000001"] img'))
    expect(m.doc.querySelector('[data-id="i-0000000001"] img').getAttribute('src')).toBe(src)
    expect(m.fake.log.urls).toHaveLength(0)
  })

  it('a line whose attached end is gone is never drawn to (0, 0)', async () => {
    const m = await started({ seed: BOARD([
      sticky('s-0000000001', { x: 600, y: 600 }),
      [P + 'obj/c-0000000002', { id: 'c-0000000002', type: 'connector', from: { id: 'h-gone000000' }, to: { id: 's-0000000001' }, route: 'straight' }],
      [P + 'obj/c-0000000003', { id: 'c-0000000003', type: 'connector', from: { id: 'h-gone000000', x: 500, y: 500 }, to: { id: 's-0000000001' }, route: 'straight' }],
      [P + 'obj/c-0000000004', { id: 'c-0000000004', type: 'connector', to: { id: 's-0000000001' }, route: 'straight' }],
    ]) })
    await until('shown', () => !!m.doc.querySelector('[data-id="c-0000000003"]'))
    const el = (id: string): Any => m.doc.querySelector('[data-id="' + id + '"]')
    // No stored point: not drawn at all, out of the board's bounds, but named for assistive tech.
    expect(el('c-0000000002').hidden).toBe(true)
    expect(m.App.R.box('c-0000000002')).toBeNull()
    expect(el('c-0000000002').getAttribute('aria-label')).toBe('Line to a deleted item')
    expect(el('c-0000000004').hidden).toBe(true)
    // A stored point: drawn faded from there.
    expect(el('c-0000000003').hidden).toBe(false)
    expect(el('c-0000000003').classList.contains('orphan')).toBe(true)
    const d = el('c-0000000003').querySelector('.line').getAttribute('d')
    expect(d.startsWith('M500 500') || d.endsWith('L500 500')).toBe(true)
    expect(d).not.toMatch(/(^|[ML])0 0\b/)
    expect(m.App.contentBox()).toEqual({ x: 500 - 0, y: expect.any(Number), w: expect.any(Number), h: expect.any(Number) })
    expect(m.App.contentBox().x).toBeGreaterThan(400)
  })

  it('exports JSON: sorted objects, person tokens, files by sha256 with their bytes, and no K2 identifiers', async () => {
    const legacy = 'data:image/png;base64,' + PNG_1x1.toString('base64')
    const m = await started({ viewer: 'Ada', seed: BOARD([
      sticky('s-00000000b2', { z: 2, reactions: { 'person:owner': ['👍'], 'agent:helper': ['🔥'], Ada: ['🎉'] }, votes: { 'agent:writer': 1, 'person:owner': 2 }, createdBy: 'agent:writer', updatedBy: 'person:owner' }),
      sticky('s-00000000a1', { z: 2, createdBy: 'Old Name', updatedBy: 'agent:helper' }),
      [P + 'obj/q-0000000003', { id: 'q-0000000003', type: 'comment', z: 1, text: 'Hm', createdBy: 'person:owner', replies: [{ by: 'agent:writer', at: 't', text: 'Yes' }, { by: 'Old Name', at: 't', text: 'No' }] }],
      [P + 'obj/i-0000000004', { id: 'i-0000000004', type: 'image', z: 0, w: 10, h: 10, src: legacy, createdBy: 'person:owner' }],
    ]) })
    await until('shown', () => !!m.App.model.get('i-0000000004'))
    const id = await m.App.addImageFile(new m.win.File([new Uint8Array(PNG_1x1)], 'p.png', { type: 'image/png' }))
    await saved(m)
    m.doc.querySelector('[aria-label="Export"]').click()
    m.doc.querySelector('#pop [aria-label="Download the board as a JSON file"]').click()
    await until('downloaded', () => m.fake.log.downloads.length === 1)
    const dl = m.fake.log.downloads[0]
    expect(dl.name).toBe('Plans.json')
    expect(dl.mime).toBe('application/json')
    const text: string = dl.data
    for (const bad of ['k2blob:', 'b0a0a0a', 'project', 'person:owner', 'agent:', 'batch']) expect(text.includes(bad), 'export has ' + bad).toBe(false)
    const env = JSON.parse(text)
    expect(Object.keys(env)).toEqual(['format', 'version', 'title', 'meta', 'objects', 'people', 'blobs'])
    expect(env).toMatchObject({ format: 'k2-board', version: 1, title: 'Plans', meta: {} })
    // Sorted by (z, id): the legacy image (0), the comment (1), the two stickies (2, by id), the new image.
    const newZ = m.App.model.get(id).z
    expect(newZ).toBeGreaterThan(2)
    expect(env.objects.map((o: Any) => o.id)).toEqual(['i-0000000004', 'q-0000000003', 's-00000000a1', 's-00000000b2', id])
    // Tokens in order of first appearance: createdBy, updatedBy, reaction keys (sorted), vote keys (sorted), reply authors.
    expect(env.people).toEqual({ p1: { name: 'Ada', kind: 'person' }, p2: { name: 'Writer', kind: 'agent' }, p3: { name: 'Old Name', kind: 'person' }, p4: { name: 'Helper', kind: 'agent' } })
    const [img, cmt, a1, b2, img2] = env.objects
    expect(img).toMatchObject({ createdBy: 'p1', src: { blob: expect.stringMatching(/^sha256-[0-9a-f]{64}$/) } })
    expect(cmt).toMatchObject({ createdBy: 'p1', replies: [{ id: 'rp-0', by: 'p2', text: 'Yes' }, { id: 'rp-1', by: 'p3', text: 'No' }] })
    expect(a1).toMatchObject({ createdBy: 'p3', updatedBy: 'p4' })
    // Ada (the viewer's older name) and person:owner are one person: one token, reactions merged in sorted-key order.
    expect(b2.reactions).toEqual({ p1: ['🎉', '👍'], p4: ['🔥'] })
    expect(Object.keys(b2.reactions)).toEqual(['p1', 'p4'])
    expect(b2.votes).toEqual({ p2: 1, p1: 2 })
    expect(b2.batch).toBeUndefined()
    // Both pictures are the same bytes: one blob, its sha256, the bytes in base64.
    const sha = webcrypto.subtle ? Buffer.from(await webcrypto.subtle.digest('SHA-256', PNG_1x1)).toString('hex') : ''
    expect(Object.keys(env.blobs)).toEqual(['sha256-' + sha])
    // The first object that names the bytes gives the entry (here the older data: picture, which has no name).
    expect(env.blobs['sha256-' + sha]).toEqual({ mime: 'image/png', name: null, data: PNG_1x1.toString('base64') })
    expect(img.src).toEqual({ blob: 'sha256-' + sha })
    expect(img2.src).toEqual({ blob: 'sha256-' + sha })
  })

  it('For agents shows the exact grant command for this board, and where else to grant', async () => {
    const m = await started({ seed: BOARD() })
    m.doc.querySelector('[aria-label="For agents: let an agent work on this board"]').click()
    const pop = m.doc.getElementById('pop')
    const cmds = [...pop.querySelectorAll('code.cmd')].map((c: Any) => c.textContent)
    expect(cmds).toEqual([
      'k2 zen data grant k2:board --agent <handle> --level write --prefix board/b0a0a0a/',
      'k2 zen data grant k2:board --agent <handle> --level read --prefix board/b0a0a0a/',
    ])
    expect(pop.textContent).toContain('Settings → Gardens → this Garden → Local Data')
    expect(pop.textContent).toContain('k2 board list --board b0a0a0a')
    expect(pop.textContent).toContain('k2 study board')
    await until('agents listed', () => pop.querySelectorAll('select.agentsel option').length === 3)
    const sel = pop.querySelector('select.agentsel'); sel.value = 'writer'; sel.dispatchEvent(new m.win.Event('change'))
    expect(pop.querySelector('code.cmd').textContent).toBe('k2 zen data grant k2:board --agent writer --level write --prefix board/b0a0a0a/')
    // Nothing about access is ever written to the board's data.
    expect([...m.fake.space.keys()].some((k: string) => /^(acl|access|settings)\//.test(k))).toBe(false)
  })

  it('the Chat column lists the agents K2 has granted this board (all local agents without that verb)', async () => {
    const grants = [
      { handle: 'writer', level: 'write', prefixes: ['board/b0a0a0a/'] },
      { handle: 'helper', level: 'read', prefixes: ['board/b9999999/'] },
      { handle: 'far', level: 'write', prefixes: null },
    ]
    const m = await started({ seed: BOARD(), grantees: grants })
    await until('shown to K2', () => m.fake.log.shown.length > 0)
    expect(m.fake.log.shown[m.fake.log.shown.length - 1]).toEqual(['writer::local'])
    await until('the board chip', () => m.fake.log.contexts[m.fake.log.contexts.length - 1] === 'Board: Plans · b0a0a0a')
    m.doc.querySelector('[aria-label="For agents: let an agent work on this board"]').click()
    await until('granted list', () => m.doc.querySelectorAll('#pop .granted .who').length === 1)
    expect(m.doc.querySelector('#pop .granted').textContent).toContain('Writer')
    // A whole-space grant (prefixes null) counts for every board.
    const w = await started({ seed: BOARD(), grantees: [{ handle: 'helper', level: 'read', prefixes: null }] })
    await until('shown', () => w.fake.log.shown.length > 0)
    expect(w.fake.log.shown[w.fake.log.shown.length - 1]).toEqual(['helper::local'])
    // No grantees verb, or K2 refuses it: every agent on this computer (never another server's).
    for (const opts of [{}, { grantees: 'refuse' }]) {
      const f = await started(Object.assign({ seed: BOARD() }, opts))
      await until('shown', () => f.fake.log.shown.length > 0)
      expect(f.fake.log.shown[f.fake.log.shown.length - 1]).toEqual(['helper::local', 'writer::local'])
    }
  })

  it('the Garden-only parts follow BoardHost.features', async () => {
    const on = await started({ seed: BOARD() })
    expect(on.doc.querySelector('[aria-label="For agents: let an agent work on this board"]')).not.toBeNull()
    expect(on.doc.querySelector('.chatbtn')).not.toBeNull()
    on.doc.querySelector('.boardname .name').click()
    expect(on.doc.getElementById('pop').classList.contains('boards')).toBe(true)
    const off = await started({ seed: BOARD(), features: { boards: false, forAgents: false, chat: false } })
    expect(off.doc.querySelector('[aria-label="For agents: let an agent work on this board"]')).toBeNull()
    expect(off.doc.querySelector('.chatbtn')).toBeNull()
    off.doc.querySelector('.boardname .name').click()
    expect(off.doc.getElementById('pop').hidden).toBe(true)
    expect(off.doc.querySelector('.boardname input.rename')).not.toBeNull()
    await sleep(50)
    expect(off.fake.log.contexts).toHaveLength(0)
    expect(off.fake.log.shown).toHaveLength(0)
  })

  it('says plainly when the Board core is off, and doesn’t keep retrying', async () => {
    const m = mount({ denyStore: 'core_off' })
    await until('the status', () => statusOf(m) === 'Board is off. Turn it on in Settings → Cores → Multiplayer')
    expect(m.fake.log.batches).toHaveLength(0)
  })

  it('any board can be deleted; when the last one goes, a new one takes its place', async () => {
    const m = await started({ seed: BOARD([sticky('s-0000000001')]) })
    const r = await m.App.store.trash('board', 'b0a0a0a')
    expect(r.saved).toBe(true)
    expect(r.created).toMatch(/^b[0-9a-f]{6}$/)
    await saved(m)
    expect(stored(m, 'boards/b0a0a0a')).toMatchObject({ trashedAt: expect.any(String), trashedBy: 'person:owner', format: 1 })
    expect(stored(m, P + 'obj/s-0000000001')).toBeDefined()   // the data stays (the Trash)
    expect(stored(m, 'boards/' + r.created)).toMatchObject({ name: 'Board', format: 1 })
    expect(stored(m, 'board/' + r.created + '/meta')).toMatchObject({ format: 1, title: 'Board' })
  })

  it('the switcher lists boards through store.sources (workspace null today, a badge when set)', async () => {
    const m = await started({ seed: BOARD([['boards/b0b0b0b', { id: 'b0b0b0b', name: 'Second', project: null, format: 1 }]]) })
    expect(m.App.store.sources.map((s: Any) => s.id)).toEqual(['garden'])
    const rows = m.App.store.sources[0].list()
    expect(rows.map((r: Any) => [r.id, r.title, r.workspace, r.trashed])).toEqual([['b0a0a0a', 'Plans', null, false], ['b0b0b0b', 'Second', null, false]])
    // A later source (board files elsewhere) shows its workspace on the row.
    m.App.store.sources.push({ id: 'files', list: () => [{ id: 'f-1', title: 'Roadmap', workspace: { id: 'w1', label: 'Design' }, project: null }], open: () => null })
    m.doc.querySelector('.boardname .name').click()
    const badges = [...m.doc.querySelectorAll('#pop .wsbadge')].map((b: Any) => b.textContent)
    expect(badges).toEqual(['Design'])
    expect(m.doc.querySelectorAll('#pop .wsbadge').length).toBe(1)
  })

  it('a board’s default agent (chat.defaultAgent) opens in the Conversation and leads the list', async () => {
    const m = await started({ seed: BOARD([['boards/b0b0b0b', { id: 'b0b0b0b', name: 'Second', project: null, format: 1 }]]) })
    expect(m.App.store.chat.defaultAgent('b0a0a0a')).toBeNull()
    expect(m.fake.log.opened).toEqual([])
    m.App.store.chat.defaultAgent = (id: string) => (id === 'b0b0b0b' ? 'writer::local' : null)
    m.App.switchBoard('b0b0b0b')
    await until('opened', () => m.fake.log.opened.includes('writer::local'))
    await until('shown', () => JSON.stringify(m.fake.log.shown[m.fake.log.shown.length - 1]) === JSON.stringify(['writer::local', 'helper::local']))
  })

  it('keeps old access data it doesn’t use, untouched, on every rewrite', async () => {
    const m = await started({ seed: BOARD([['acl/board/b0a0a0a', { access: { helper: 'edit' } }], ['access/b0a0a0a', { agents: [] }]]).map(([k, v]) => (k === 'boards/b0a0a0a' ? [k, Object.assign({}, v as object, { access: { helper: 'edit' }, accessApplied: { helper: 'edit' } })] : [k, v])) as Array<[string, unknown]> })
    m.App.store.rename('b0a0a0a', 'Renamed')
    await saved(m)
    expect(stored(m, 'boards/b0a0a0a')).toEqual({ id: 'b0a0a0a', name: 'Renamed', project: null, format: 1, access: { helper: 'edit' }, accessApplied: { helper: 'edit' } })
    expect(stored(m, 'acl/board/b0a0a0a')).toEqual({ access: { helper: 'edit' } })
    expect(stored(m, 'access/b0a0a0a')).toEqual({ agents: [] })
  })
})
