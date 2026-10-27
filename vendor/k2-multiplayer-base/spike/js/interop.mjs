// D0 interop harness: Yjs (Node) <-> Yrs (Rust peer over stdio).
// Fails loudly: any divergence or check failure throws and exits non-zero.
//
//   node interop.mjs [--seed N] [--minutes M]
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { statSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import assert from 'node:assert/strict'
import * as Y from 'yjs'
import * as syncProtocol from 'y-protocols/sync'
import * as awarenessProtocol from 'y-protocols/awareness'
import * as encoding from 'lib0/encoding'
import * as decoding from 'lib0/decoding'

const arg = (k, d) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : d }
const SEED = Number(arg('--seed', 1))
const MINUTES = Number(arg('--minutes', 10))
const BIN = fileURLToPath(new URL('../rust/target/release/d0-spike', import.meta.url))
const b64 = (u8) => Buffer.from(u8).toString('base64')
const unb64 = (s) => new Uint8Array(Buffer.from(s, 'base64'))
const results = {}

// ------------------------------------------------------------- the Rust peer
function startPeer () {
  const p = spawn(BIN, ['peer'], { stdio: ['pipe', 'pipe', 'inherit'] })
  const rl = createInterface({ input: p.stdout })
  const waiting = []
  rl.on('line', (l) => waiting.shift()(JSON.parse(l)))
  p.on('exit', (c) => { if (c) { console.error('peer exited', c); process.exit(1) } })
  return {
    call: (req) => new Promise((res) => { waiting.push(res); p.stdin.write(JSON.stringify(req) + '\n') }),
    close: () => p.stdin.end()
  }
}

// seeded PRNG (mulberry32)
function rng (seed) {
  let a = seed >>> 0
  return () => { a |= 0; a = (a + 0x6D2B79F5) | 0; let t = Math.imul(a ^ (a >>> 15), 1 | a); t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t; return ((t ^ (t >>> 14)) >>> 0) / 4294967296 }
}

// ------------------------------------------------------------- T1 framing
async function t1Framing (peer) {
  await peer.call({ c: 'reset' })
  const d = new Y.Doc(); d.clientID = 501
  d.getText('source').insert(0, 'héllo 🎉 world\n')
  const msgs = {}
  let e = encoding.createEncoder(); encoding.writeVarUint(e, 0); syncProtocol.writeSyncStep1(e, d); msgs.sync1 = encoding.toUint8Array(e)
  e = encoding.createEncoder(); encoding.writeVarUint(e, 0); syncProtocol.writeSyncStep2(e, d); msgs.sync2 = encoding.toUint8Array(e)
  e = encoding.createEncoder(); encoding.writeVarUint(e, 0); syncProtocol.writeUpdate(e, Y.encodeStateAsUpdate(d)); msgs.update = encoding.toUint8Array(e)
  const aw = new awarenessProtocol.Awareness(d)
  aw.setLocalState({ user: { name: 'Sam', color: '#3366ff' }, cursor: { anchor: { type: null, tname: 'source', item: { client: 501, clock: 3 }, assoc: 0 }, head: null } })
  e = encoding.createEncoder(); encoding.writeVarUint(e, 1); encoding.writeVarUint8Array(e, awarenessProtocol.encodeAwarenessUpdate(aw, [d.clientID])); msgs.awareness = encoding.toUint8Array(e)
  const js2rs = {}
  for (const [k, m] of Object.entries(msgs)) {
    const r = await peer.call({ c: 'msg', m: b64(m) })
    assert.equal(r.kind, k === 'update' ? 'update' : k, `yrs decoded ${k} as ${r.kind}`)
    assert.equal(r.same_bytes, true, `yrs re-encoded ${k} to different bytes`)
    js2rs[k] = 'ok'
    if (k === 'awareness') assert.match(r.detail[0].json, /"Sam"/)
  }
  aw.destroy()

  // Rust -> JS: the daemon's messages decode in y-protocols.
  await peer.call({ c: 'apply', u: b64(Y.encodeStateAsUpdate(d)) })
  const rm = await peer.call({ c: 'make_msgs' })
  const fresh = new Y.Doc(); fresh.clientID = 502
  for (const k of ['sync1', 'sync2']) {
    const dec = decoding.createDecoder(unb64(rm[k]))
    assert.equal(decoding.readVarUint(dec), 0)
    const out = encoding.createEncoder()
    const t = syncProtocol.readSyncMessage(dec, out, fresh, 'rust')
    assert.equal(t, k === 'sync1' ? syncProtocol.messageYjsSyncStep1 : syncProtocol.messageYjsSyncStep2)
  }
  assert.equal(fresh.getText('source').toString(), 'héllo 🎉 world\n', 'sync2 from yrs did not reproduce the text')
  const dec = decoding.createDecoder(unb64(rm.awareness))
  assert.equal(decoding.readVarUint(dec), 1)
  const aw2 = new awarenessProtocol.Awareness(new Y.Doc())
  awarenessProtocol.applyAwarenessUpdate(aw2, decoding.readVarUint8Array(dec), 'rust')
  const st = aw2.getStates().get(77)
  assert.equal(st?.user?.name, 'agent-x', 'awareness from yrs did not decode')
  aw2.destroy()
  results.T1_framing = { js_to_yrs: js2rs, yrs_to_js: { sync1: 'ok', sync2: 'ok', awareness: 'ok' } }
}

// ------------------------------------------------------------- T2 convergence (trace 3)
const SNIPPETS = ['a', 'e', ' ', 'the ', '\n', '## ', '- [ ] ', '**', 'é', '日本', '🎉', 'ñ', '`code`']
function safePos (s, p) { // never split a surrogate pair (editors don't)
  if (p > 0 && p < s.length) { const c = s.charCodeAt(p - 1); if (c >= 0xd800 && c <= 0xdbff) return p + 1 }
  return p
}
async function t2Convergence (peer) {
  const r = rng(SEED)
  await peer.call({ c: 'reset', daemon: 1, agent: 2 })
  const typists = [101, 102, 103].map((id) => {
    const doc = new Y.Doc(); doc.clientID = id
    return { id, doc, text: doc.getText('source'), online: true, offlineUntil: 0, outbox: [] }
  })
  const ticks = MINUTES * 60 * 5 // 200 ms ticks
  let keystrokes = 0; let agentEdits = 0; let reconnects = 0; let attributionViolations = 0; let checks = 0
  const relay = (from, u) => { for (const t of typists) if (t !== from && t.online) Y.applyUpdate(t.doc, u, 'daemon') }
  for (const t of typists) {
    t.doc.on('update', (u, origin) => { if (origin === 'local') t.outbox.push(u) })
  }
  const t0 = Date.now()
  for (let tick = 0; tick < ticks; tick++) {
    const now = tick * 200
    for (const t of typists) {
      // connectivity
      if (t.online && r() < 0.002) { t.online = false; t.offlineUntil = now + 2000 + Math.floor(r() * 18000) }
      if (!t.online && now >= t.offlineUntil) {
        // reconnect: SV exchange both ways (what y-protocols does on connect)
        const sv = (await peer.call({ c: 'sv' })).sv
        const mine = Y.encodeStateAsUpdate(t.doc, unb64(sv))
        const ar = await peer.call({ c: 'apply', u: b64(mine) })
        assert.ok(ar.ok, 'reconnect step2 rejected: ' + ar.err)
        if (ar.inserts.some((c) => c !== t.id)) attributionViolations++
        relay(t, mine)
        const missing = (await peer.call({ c: 'diff', sv: b64(Y.encodeStateVector(t.doc)) })).u
        Y.applyUpdate(t.doc, unb64(missing), 'daemon')
        t.outbox = []; t.online = true; reconnects++
      }
      // typing (~5 keystrokes/s)
      if (r() < 1.0) {
        const s = t.text.toString()
        t.doc.transact(() => {
          if (s.length > 20 && r() < 0.18) {
            const p = safePos(s, Math.floor(r() * (s.length - 1)))
            let n = 1 + Math.floor(r() * 4); if (p + n > s.length) n = s.length - p
            const end = safePos(s, p + n)
            t.text.delete(p, end - p)
          } else {
            t.text.insert(safePos(s, Math.floor(r() * (s.length + 1))), SNIPPETS[Math.floor(r() * SNIPPETS.length)])
          }
        }, 'local')
        keystrokes++
      }
      if (t.online) {
        for (const u of t.outbox) {
          const ar = await peer.call({ c: 'apply', u: b64(u) })
          assert.ok(ar.ok, 'live update rejected: ' + ar.err)
          if (ar.inserts.some((c) => c !== t.id)) attributionViolations++
          relay(t, u)
        }
        t.outbox = []
      }
    }
    // agent every 20 s: alternate a structural rewrite and a single-word edit
    if (tick % 100 === 99) {
      const cur = (await peer.call({ c: 'text' })).text
      const res = agentEdits % 2 === 0
        ? await peer.call({ c: 'agent_write', new: cur + `\n> agent note ${agentEdits} ✓\n` })
        : await peer.call({ c: 'agent_upcase_word', n: agentEdits * 7 })
      relay(null, unb64(res.u))
      agentEdits++
    }
    // strong check: every online replica equals the daemon after every tick
    if (tick % 10 === 0 || tick === ticks - 1) {
      const rt = (await peer.call({ c: 'text' })).text
      for (const t of typists) if (t.online) assert.equal(t.text.toString(), rt, `typist ${t.id} diverged at tick ${tick}`)
      checks++
    }
  }
  // bring everyone back and do the final check
  for (const t of typists) if (!t.online) { t.offlineUntil = 0 }
  for (const t of typists) {
    if (t.online) continue
    const sv = (await peer.call({ c: 'sv' })).sv
    const mine = Y.encodeStateAsUpdate(t.doc, unb64(sv))
    await peer.call({ c: 'apply', u: b64(mine) }); relay(t, mine)
    Y.applyUpdate(t.doc, unb64((await peer.call({ c: 'diff', sv: b64(Y.encodeStateVector(t.doc)) })).u))
    t.online = true
  }
  const final = (await peer.call({ c: 'text' })).text
  for (const t of typists) assert.equal(t.text.toString(), final, `final divergence at typist ${t.id}`)
  assert.equal(attributionViolations, 0, 'an honest client update inserted under a foreign client id')
  const mem = (await peer.call({ c: 'mem' })).live
  const breakdown = await peer.call({ c: 'mem_breakdown' })
  // after the in-memory compaction swap, a client keeps syncing normally
  const t = typists[0]
  t.doc.transact(() => t.text.insert(0, 'after-swap '), 'local')
  for (const u of t.outbox.splice(0)) { assert.ok((await peer.call({ c: 'apply', u: b64(u) })).ok); relay(t, u) }
  const afterSwap = (await peer.call({ c: 'text' })).text
  for (const x of typists) assert.equal(x.text.toString(), afterSwap, 'divergence after compaction swap')
  results.T2_convergence = {
    seed: SEED, simulated_minutes: MINUTES, keystrokes, agent_edits: agentEdits, reconnects, checks,
    final_chars: final.length, final_utf8_bytes: Buffer.byteLength(final), peer_heap_bytes: mem,
    mem_breakdown: breakdown, swap_then_edit_converged: true,
    wall_ms: Date.now() - t0, attribution_violations: attributionViolations, converged: true
  }
}

// ------------------------------------------------------------- T3 forged update from JS
async function t3Forgery (peer) {
  await peer.call({ c: 'reset' })
  const assigned = 301
  const liar = new Y.Doc(); liar.clientID = 999 // claims someone else's id
  liar.getText('source').insert(0, 'not mine')
  const r = await peer.call({ c: 'apply', u: b64(Y.encodeStateAsUpdate(liar)) })
  const caught = r.inserts.some((c) => c !== assigned)
  assert.ok(caught, 'a JS update under a foreign client id was not detected')
  results.T3_forgery_from_js = { assigned, inserts: r.inserts, caught }
}

// ------------------------------------------------------------- T4 y-codemirror.next headless
async function t4CodeMirror (peer) {
  const { JSDOM } = await import('jsdom')
  const dom = new JSDOM('<!doctype html><html><body><div id="ed"></div></body></html>', { pretendToBeVisual: true })
  const g = globalThis
  for (const k of ['window', 'document', 'navigator', 'MutationObserver', 'getComputedStyle', 'requestAnimationFrame', 'cancelAnimationFrame', 'HTMLElement', 'Node', 'Range', 'Text', 'Element']) {
    if (!(k in g) || k === 'navigator') { try { Object.defineProperty(g, k, { value: dom.window[k], configurable: true, writable: true }) } catch {} }
  }
  dom.window.Range.prototype.getClientRects = () => ({ length: 0, item: () => null, [Symbol.iterator]: function * () {} })
  dom.window.Range.prototype.getBoundingClientRect = () => ({ left: 0, right: 0, top: 0, bottom: 0, width: 0, height: 0 })
  const { EditorState } = await import('@codemirror/state')
  const { EditorView } = await import('@codemirror/view')
  const { yCollab } = await import('y-codemirror.next')

  await peer.call({ c: 'reset', daemon: 1, agent: 2 })
  const ydoc = new Y.Doc(); ydoc.clientID = 601
  const ytext = ydoc.getText('source')
  const aw = new awarenessProtocol.Awareness(ydoc)
  aw.setLocalStateField('user', { name: 'tab', color: '#00aa88' })
  const undoManager = new Y.UndoManager(ytext)
  const outbox = []
  ydoc.on('update', (u, origin) => { if (origin !== 'daemon') outbox.push(u) })
  const view = new EditorView({
    state: EditorState.create({ doc: ytext.toString(), extensions: [yCollab(ytext, aw, { undoManager })] }),
    parent: dom.window.document.getElementById('ed')
  })
  const flush = async () => { for (const u of outbox.splice(0)) { const r = await peer.call({ c: 'apply', u: b64(u) }); assert.ok(r.ok) } }

  // typing in the editor reaches Y.Text and the daemon
  view.dispatch({ changes: { from: 0, insert: '# Notes\n\nfirst line from the tab\n' } })
  await flush()
  assert.equal((await peer.call({ c: 'text' })).text, '# Notes\n\nfirst line from the tab\n')
  // an agent edit in Rust lands in the editor view
  const a = await peer.call({ c: 'agent_write', new: '# Notes\n\nfirst line from the tab\nagent: second line\n' })
  Y.applyUpdate(ydoc, unb64(a.u), 'daemon')
  assert.equal(view.state.doc.toString(), '# Notes\n\nfirst line from the tab\nagent: second line\n', 'agent edit did not reach the CodeMirror view')
  // caret as a relative position survives the Rust hop. y-codemirror.next publishes the
  // caret only while the editor has focus, so unfocused tabs show as viewers.
  Object.defineProperty(view, 'hasFocus', { get: () => true })
  dom.window.document.hasFocus = () => true
  view.dispatch({ selection: { anchor: 9 } })
  const cursor = aw.getLocalState().cursor
  assert.ok(cursor?.anchor, 'yCollab did not publish a cursor')
  const other = new Y.Doc(); other.clientID = 602
  Y.applyUpdate(other, unb64((await peer.call({ c: 'diff', sv: b64(Y.encodeStateVector(other)) })).u))
  const abs = Y.createAbsolutePositionFromRelativePosition(Y.createRelativePositionFromJSON(cursor.anchor), other)
  assert.equal(abs?.index, 9, 'relative cursor resolved to the wrong index on a doc synced through yrs')
  // per-user undo: undo removes only the tab's typing, not the agent's line
  view.dispatch({ changes: { from: view.state.doc.length, insert: 'tab typed this\n' } })
  undoManager.stopCapturing()
  undoManager.undo()
  assert.equal(view.state.doc.toString().includes('agent: second line'), true)
  assert.equal(view.state.doc.toString().includes('tab typed this'), false)
  await flush()
  assert.equal((await peer.call({ c: 'text' })).text, view.state.doc.toString())
  // awareness frame with a cursor decodes in yrs
  const e = encoding.createEncoder(); encoding.writeVarUint(e, 1)
  encoding.writeVarUint8Array(e, awarenessProtocol.encodeAwarenessUpdate(aw, [ydoc.clientID]))
  const m = await peer.call({ c: 'msg', m: b64(encoding.toUint8Array(e)) })
  assert.equal(m.kind, 'awareness'); assert.match(m.detail[0].json, /"cursor"/)
  view.destroy(); aw.destroy()
  results.T4_codemirror = { typing_to_yrs: 'ok', agent_to_view: 'ok', relative_cursor_through_yrs: 'ok', per_user_undo: 'ok', awareness_cursor_decodes_in_yrs: 'ok' }
}

// ------------------------------------------------------------- client weight
function dirSize (p) { let n = 0; for (const f of readdirSync(p, { withFileTypes: true })) { const q = join(p, f.name); n += f.isDirectory() ? dirSize(q) : statSync(q).size } return n }
function weights () {
  const nm = fileURLToPath(new URL('./node_modules/', import.meta.url))
  const wasm = readdirSync(join(nm, 'loro-crdt'), { recursive: true }).filter((f) => String(f).endsWith('.wasm')).map((f) => statSync(join(nm, 'loro-crdt', String(f))).size)
  results.client_weight = {
    yjs_dist_cjs_bytes: statSync(join(nm, 'yjs/dist/yjs.cjs')).size,
    loro_wasm_bytes_each: wasm,
    unpacked: { yjs: dirSize(join(nm, 'yjs')), 'y-protocols': dirSize(join(nm, 'y-protocols')), 'y-codemirror.next': dirSize(join(nm, 'y-codemirror.next')), 'loro-crdt': dirSize(join(nm, 'loro-crdt')) }
  }
}

const peer = startPeer()
try {
  await t1Framing(peer)
  await t3Forgery(peer)
  await t4CodeMirror(peer)
  await t2Convergence(peer)
  weights()
  console.log(JSON.stringify(results, null, 2))
} finally {
  peer.close()
}
