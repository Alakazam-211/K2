// BoardStore for a .k5board in a K2 tab: the only file here that knows how Board saves and loads. The file's
// truth is K2's: this frame never sees zip bytes and never builds board.json. It sends each gesture's ops to the
// tab (k2.boardFile.submit), which queues them outside the frame, and hears what changes (k2.boardFile.events).
// A person key is the file's own token. Objects come in the widget's shape; a field this widget can't show isn't
// shown, and stays in the file, because K2 applies only the fields an op names.
// The UI's contract is the same as board-1/store.js; the Garden-only parts answer nothing.
(function (g) {
  'use strict'
  const S = g.BoardSchema
  const FILE = 'file'        // the one "board" a tab has
  const PART = 30000         // chars of JSON per message: K2's 64 KB limit, with room for escapes
  const OWN = ['createdBy', 'createdAt', 'updatedBy', 'updatedAt']
  const isObj = (v) => !!v && typeof v === 'object' && !Array.isArray(v)
  const clone = (v) => (v === undefined ? undefined : JSON.parse(JSON.stringify(v)))
  const hex = (n) => { const a = new Uint8Array(n); g.crypto.getRandomValues(a); return Array.from(a, (b) => b.toString(16).padStart(2, '0')).join('') }
  const rid = (p) => p + '-' + hex(5)

  // An object as read → one the model can show. A field that doesn't pass the widget's schema isn't shown.
  function toObj(v) {
    if (!isObj(v) || typeof v.id !== 'string' || typeof v.type !== 'string') return null
    if (!S.TYPES[v.type]) return Object.assign({ x: 0, y: 0, w: 240, h: 120, z: 0 }, clone(v), { foreign: true })   // BT-21: kept, drawn, locked
    const o = Object.assign({ id: v.id, type: v.type, x: 0, y: 0, z: 0 }, clone(S.DEFAULTS[v.type] || {}))
    for (const k of Object.keys(v)) {
      if (k === 'id' || k === 'type') continue
      if (OWN.includes(k)) { if (typeof v[k] === 'string') o[k] = v[k]; continue }
      if (S.isExtraKey(v.type, k)) { Object.assign(o, S.splitExtra(v.type, { [k]: v[k] }).extra); continue }
      let f = null
      try { f = S.clean(v.type, { [k]: v[k] }) } catch (e) { if (!S.isRefusal(e)) throw e }
      if (f) Object.assign(o, f)
    }
    for (const k of ['checks', 'crop', 'link']) if (o[k] === null) delete o[k]
    return S.ensureIds(o)
  }
  // The fields of a remote `set` the widget can show, on an object it holds.
  function cleanFields(type, f) {
    if (!S.TYPES[type]) return clone(f)
    const out = {}
    for (const k of Object.keys(f || {})) {
      if (f[k] === null || S.isExtraKey(type, k)) { out[k] = f[k]; continue }
      try { Object.assign(out, S.clean(type, { [k]: f[k] })) } catch (e) { if (!S.isRefusal(e)) throw e }
    }
    return out
  }

  function create() {
    let model = null; let me = ''; let people = {}; let name = 'Board'; let open = null; let boardId = null
    let readOnly = false; let onReadOnly = () => {}; let replay = () => {}; let holdFn = () => false; let held = false
    let status = { state: 'pending', text: 'Loading…' }; const listeners = []
    let sendQ = Promise.resolve(); let stopEvents = null; const urls = new Map()
    const prefs = {}
    const setStatus = (st) => { status = st; listeners.forEach((f) => f(status)) }
    const app = () => g.BoardApp

    // Where an object now sits among its kind, in the words `save` takes (order never has a rev).
    function placeOf(id, field, kinds) {
      const all = model.all().filter((o) => kinds(o) && typeof o[field] === 'number').sort((a, b) => a[field] - b[field])
      const i = all.findIndex((o) => o.id === id)
      if (i < 0 || i === all.length - 1) return 'front'
      if (i === 0) return 'back'
      return { after: all.slice(Math.max(0, i - 8), i).reverse().map((o) => o.id) }
    }
    const isFrame = (o) => o.type === 'frame'
    const TABLE = ['rows', 'rowIds', 'colIds', 'colW']

    // A gesture's ops → the ops `save` takes. The model has already applied them, so positions are final.
    // `z` and `order` beside an op are for the tab's own copy of the board; it never sends them.
    function toWire(ops) {
      const out = []; const moves = []
      for (const op of ops) {
        if (op.op === 'add') {
          const obj = clone(op.obj); delete obj.z; delete obj.order
          const now = model.get(obj.id)
          const add = { op: 'add', obj, at: placeOf(obj.id, 'z', () => true), z: now ? now.z : 0 }
          if (now && obj.type === 'frame') add.order = now.order
          out.push(add)
          if (obj.type === 'frame') moves.push({ op: 'frame', id: obj.id })
        } else if (op.op === 'set') {
          const f = clone(op.f) || {}; const o = model.get(op.id)
          if ('z' in f) { delete f.z; moves.push({ op: 'z', id: op.id }) }
          if ('order' in f) { delete f.order; moves.push({ op: 'frame', id: op.id }) }
          if (o && o.type === 'table' && TABLE.some((k) => k in f)) for (const k of TABLE) if (o[k] !== undefined) f[k] = clone(o[k])
          if (Object.keys(f).length) out.push({ op: 'set', id: op.id, f })
        } else if (op.op === 'del') {
          out.push({ op: 'del', id: op.id })
        } else if (op.op === 'meta') {
          const f = clone(op.f) || {}; delete f.timer
          if (Object.keys(f).length) out.push({ op: 'meta', f })
        }
      }
      // Lowest first, so each one lands above the ones that moved before it.
      const at = (m) => { const o = model.get(m.id); return o ? (m.op === 'z' ? o.z : o.order) || 0 : 0 }
      for (const m of moves.sort((a, b) => at(a) - at(b))) {
        const o = model.get(m.id); if (!o) continue
        out.push(m.op === 'z' ? { op: 'z', id: m.id, to: placeOf(m.id, 'z', () => true), z: o.z } : { op: 'frame', id: m.id, to: placeOf(m.id, 'order', isFrame), order: o.order })
      }
      return out
    }

    function send(info, wire) {
      const text = JSON.stringify(wire); const parts = Math.max(1, Math.ceil(text.length / PART))
      for (let i = 0; i < parts; i++) {
        const body = { gesture: info.gesture, kind: info.kind, byAll: !!info.byAll, part: i, last: i === parts - 1, text: text.slice(i * PART, (i + 1) * PART) }
        sendQ = sendQ.then(() => g.k2.boardFile.submit(body)).catch((e) => {
          setStatus({ state: 'error', text: 'Not saved: this tab lost K2 (' + ((e && e.code) || 'error') + ')' })
        })
      }
    }

    function onEvent(ev) {
      if (!isObj(ev)) return
      if (ev.type === 'remote') {
        const ops = []
        for (const op of ev.ops || []) {
          if (op.op === 'add') { const o = toObj(op.obj); if (o) ops.push({ op: 'add', obj: o }) }
          else if (op.op === 'set') { const o = model.get(op.id); if (o) ops.push({ op: 'set', id: op.id, f: cleanFields(o.type, op.f) }) }
          else if (op.op === 'meta') { const f = clone(op.f) || {}; delete f.timer; if (Object.keys(f).length) ops.push({ op: 'meta', f }) }
          else ops.push(op)
        }
        if (ops.length) replay({ by: ev.by || 'someone', at: new Date().toISOString(), ops })
      } else if (ev.type === 'status') setStatus({ state: ev.state, text: ev.text, detail: ev.detail })
      else if (ev.type === 'readOnly') { readOnly = !!ev.on; onReadOnly(readOnly, ev.why || '') }
      else if (ev.type === 'people') { if (ev.me && ev.me !== me) { me = ev.me; model.setMe(me) } people = ev.people || people }
      else if (ev.type === 'refused') { model.rollback(ev.gesture); if (app()) app().toast(ev.text || 'K2 couldn’t save that change, so it was undone') }
      else if (ev.type === 'historyRefused') { model.historyRefused(ev.gesture); if (app()) app().toast('Someone changed this since; undo can’t apply') }
      else if (ev.type === 'saved') { for (const t of ev.tokens || []) model.forget(t) }
      else if (ev.type === 'focus') { if (app()) app().showObject(ev.id) }
    }

    const api = {
      rid,
      get me() { return me },
      isMe: (k) => k === me,
      isAgent: (k) => !!(people[k] && people[k].kind === 'agent'),
      nameOf: (k) => (k === me ? 'You' : (people[k] && people[k].name) || 'Someone'),
      samePerson: (a, b) => a === b,
      get viewerName() { return (people[me] && people[me].name) || 'You' },
      bindModel(m) { model = m },
      load: () => ({ boardId: null, board: { objects: {}, meta: {} } }),
      setHold(fn) { holdFn = fn },
      async start(applyRemote) {
        replay = applyRemote
        await g.k2.connected
        open = await g.k2.boardFile.open()
        name = typeof open.name === 'string' && open.name ? open.name : 'Board'
        me = open.me || ''; people = open.people || {}
        model.setMe(me)
        stopEvents = g.k2.boardFile.events(onEvent, (e) => setStatus({ state: 'error', text: 'Live updates stopped (' + ((e && e.code) || 'error') + ')' }))
        setInterval(() => { const h = !!holdFn(); if (h !== held) { held = h; g.k2.boardFile.hold(h).catch(() => {}) } }, 250)
        if (open.readOnly) { readOnly = true; onReadOnly(true, open.readOnly) }
        setStatus(open.status || { state: 'saved', text: 'Saved' })
      },
      submit(ops, info) { const wire = toWire(ops); if (wire.length) send(info || { gesture: rid('t'), kind: 'edit' }, wire) },
      boards: () => [{ id: FILE, name }],
      get boardId() { return boardId },
      get readOnly() { return readOnly },
      onReadOnly(cb) { onReadOnly = cb },
      open(id) {
        if (id !== FILE || boardId === FILE || !open) return null
        boardId = FILE
        const objects = {}
        for (const v of open.objects || []) { const o = toObj(v); if (o) objects[o.id] = o }
        const board = { objects, meta: clone(open.meta) || {} }
        delete board.meta.timer   // never in the format: the widget's timer writes as it's drawn, so it stays out (V35)
        open = null
        return { board, later: [] }
      },
      // The Garden's board list: a tab has one file, and renames it in the drawer.
      create() { return null }, rename() {}, projects: () => [], renameProject() {}, createProject() { return null }, nest: async () => ({ saved: false, refused: true }), reorder() {}, move: async () => ({ unchanged: true }), trash: async () => ({ saved: false }), restore: async () => ({ saved: false }),
      prefs: () => Object.assign({}, prefs),
      setPref(k, v) { prefs[k] = v },
      restored: () => ({ board: FILE }),
      flushNow() { sendQ = sendQ.then(() => g.k2.boardFile.flush()).catch(() => {}) },
      onStatus(cb) { listeners.push(cb); cb(status) },
      canBlobs: () => typeof g.k2.can === 'function' && g.k2.can('blobs.put'),
      putBlob: (file, nm) => g.k2.blobs.put(file, nm ? { name: String(nm).slice(0, 255) } : undefined),
      // One full image per file: K2 has no thumbnails for a board file yet. A part that isn't a picture is refused,
      // and the image shows its placeholder.
      blobUrl(ref) { if (!urls.has(ref)) urls.set(ref, g.k2.blobs.url(ref).catch((e) => { urls.delete(ref); throw e })); return urls.get(ref) },
      blobGet: (ref) => g.k2.blobs.get(ref),
      history: async () => [],
      exportSource: async () => { throw Object.assign(new Error('a board file has no JSON export'), { code: 'not_here' }) },
      dispose() { if (stopEvents) stopEvents() },
    }
    api.sources = [{ id: 'file', list: () => [{ id: FILE, title: name, workspace: null, project: null, order: 0, trashed: false, trashedAt: null, trashedWith: null, agentEdit: false }], open: (id) => api.open(id) }]
    return api
  }

  g.BoardStore = { create }
})(window)
