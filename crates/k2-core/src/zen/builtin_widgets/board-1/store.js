// BoardStore: the only file that knows how Board saves and loads (with host.js, the only one that calls K2).
// Boards live in this widget's K2 Garden data (the store), one key per object:
//   seeded                    {at}: written once, with the first board (two windows on a fresh box make one board)
//   boards/<b>                {id, name, project, order?, format: 1, trashedAt?, trashedWith?, trashedBy?}
//   projects/<p>              {id, name, parent?, order?, trashedAt?, trashedWith?, trashedBy?}
//   board/<b>/meta            {format: 1, title, voting?, timer?, batch?}
//   board/<b>/obj/<id>        the object: schema fields plus createdBy/At, updatedBy/At and batch (the write that last touched it)
// Keys and fields this build doesn't know are never read for decisions, never deleted, and kept on every rewrite
// (a write is the stored value with the changed fields merged in).
// People are keyed by a stable id: person:owner (the person at this computer) or agent:<handle>.
// The UI uses:
//   store.load() → {boardId, board}          store.start(applyRemote) → Promise
//   store.submit(ops)                        store.onStatus(cb) {state: 'saved'|'pending'|'saving'|'error', text, detail}
//   store.boards() / projects()              store.open(id) → {board, later} | null
//   store.create(name, project) → id         store.rename(id, name), renameProject, createProject, nest, reorder, move
//   store.trash(kind, id) / restore(kind, id) → Promise<{saved, created?}>
//   store.prefs() / setPref(k, v) / restored()   the person's own settings (light/dark, the open board): the private save 'prefs'
//   store.me / isMe(key) / nameOf(key)       people
//   store.putBlob(file, name) / blobUrl(ref, px) / blobGet(ref)   images
//   store.exportSource() → {meta, objects}   the open board as stored, for the JSON export
//   store.chat                               the Garden's Chat column (K2's own Conversation beside the board)
(function (g) {
  'use strict'
  const S = g.BoardSchema
  const ME = 'person:owner'
  const IDLE_MS = 400        // write this long after the last edit (a drag is one write)
  const BATCH_MAX = 64       // ops per k2.store.batch; more go in one transaction
  const TX_MAX = 1000        // ops per transaction
  const KEY_RE = /^board\/([^/]+)\/(?:obj\/([^/]+)|(meta))$/
  const REG_RE = /^boards\/([^/]+)$/
  const PROJ_RE = /^projects\/([^/]+)$/
  const OWN = ['batch', 'createdBy', 'createdAt', 'updatedBy', 'updatedAt']   // kept with an object, not schema fields
  const OFF_TEXT = 'Board is off. Turn it on in Settings → Cores → Multiplayer'
  const codeOf = (e) => (e && (e.code || e.message)) || 'error'
  const seg = (v) => String(v).replace(/[^A-Za-z0-9._~:@+-]/g, '_')   // a key segment
  const isObj = (v) => !!v && typeof v === 'object' && !Array.isArray(v)
  const clone = (v) => (v === undefined ? undefined : JSON.parse(JSON.stringify(v)))
  const hex = (n) => { const a = new Uint8Array(n); g.crypto.getRandomValues(a); return Array.from(a, (b) => b.toString(16).padStart(2, '0')).join('') }
  const rid = (p) => p + '-' + hex(5)
  const may = (verb) => typeof g.k2.can === 'function' && g.k2.can(verb)
  const later = (ms, v) => new Promise((res) => setTimeout(() => res(v), ms))

  // ---------- people ----------
  let viewer = ''                 // K2's name for the person (Settings' name), '' until known
  const agentRows = new Map()     // handle (lowercase) → {handle, label, address, image, color}
  // K2's `by` on a change: you, you via <widget>, or agent:<handle>.
  function personOf(by) {
    if (typeof by !== 'string' || !by) return null
    if (by === 'you' || by.indexOf('you via ') === 0) return ME
    if (by.indexOf('agent:') === 0) return 'agent:' + by.slice(6).toLowerCase()
    return null
  }
  // A stored person value: person:owner, agent:<handle>, or an older plain name (shown as it is).
  const isMe = (k) => k === ME || (typeof k === 'string' && k.indexOf(':') < 0 && !!viewer && k.toLowerCase() === viewer.toLowerCase())
  const isAgent = (k) => typeof k === 'string' && k.indexOf('agent:') === 0
  function nameOf(k) {
    if (isMe(k) || k === 'you' || (typeof k === 'string' && k.indexOf('you via ') === 0)) return viewer || 'You'
    if (isAgent(k)) { const h = k.slice(6); const r = agentRows.get(h.toLowerCase()); return (r && r.label) || h }
    return typeof k === 'string' && k ? k : 'Someone'
  }
  // Two person values are the same person: the same key, or older plain names that differ only in case.
  const samePerson = (a, b) => a === b || (isMe(a) && isMe(b)) || (typeof a === 'string' && typeof b === 'string' && a.indexOf(':') < 0 && b.indexOf(':') < 0 && a.toLowerCase() === b.toLowerCase())
  // An agent's picture: only a PNG, JPEG, WebP or GIF data: URL (never SVG), else null.
  const picture = (v) => (typeof v === 'string' && /^data:image\/(png|jpeg|webp|gif);base64,[A-Za-z0-9+/=]+$/.test(v) && v.length < 360000 ? v : null)
  async function localAgents() {
    let rows = []
    try { rows = await g.k2.agents.list() } catch (_) { return [] }
    const out = (Array.isArray(rows) ? rows : []).filter((r) => r && r.server == null && typeof r.address === 'string')
      .map((r) => { const handle = r.address.split('::')[0]; return { handle, label: typeof r.label === 'string' && r.label ? r.label : handle, address: r.address, image: picture(r.image), color: typeof r.color === 'string' && /^#[0-9a-f]{6}$/i.test(r.color) ? r.color : null } })
    for (const a of out) agentRows.set(a.handle.toLowerCase(), a)
    return out
  }
  async function loadPeople() {
    const v = await Promise.race([g.k2.viewer().catch(() => null), later(1500, null)])   // never hold up loading
    const n = v && typeof v.name === 'string' ? v.name.replace(/[\u0000-\u001f\u007f]+/g, ' ').trim() : ''
    viewer = n && n.toLowerCase() !== 'you' ? n : ''
    await Promise.race([localAgents(), later(1500)])
  }

  // Merge a new op into what's pending, so a drag or a typing burst is one op.
  // Each pending op carries its board (bd), so edits made before a switch still go to their own board.
  function mergeOp(pending, op, boardId) {
    const key = op.op === 'add' ? op.obj.id : op.id
    if (op.op === 'set') {
      const add = pending.find((p) => p.op === 'add' && p.obj.id === key && p.bd === boardId)
      if (add) { Object.assign(add.obj, clone(op.f)); return pending }
      const prev = pending.find((p) => p.op === 'set' && p.id === key && p.bd === boardId)
      if (prev) { Object.assign(prev.f, clone(op.f)); return pending }
    } else if (op.op === 'del') {
      // Made in this same pending window: it never reached the store, so it all goes. But if an earlier
      // pending delete removed a stored object before that add (delete → undo → delete), a delete still goes out.
      const addAt = pending.findIndex((p) => p.op === 'add' && p.obj.id === key && p.bd === boardId)
      const savedBefore = addAt >= 0 && pending.slice(0, addAt).some((p) => p.op === 'del' && p.id === key && p.bd === boardId)
      pending = pending.filter((p) => p.bd !== boardId || (p.op === 'add' ? p.obj.id : p.id) !== key)
      if (addAt >= 0 && !savedBefore) return pending
    } else if (op.op === 'meta') {
      const prev = pending.find((p) => p.op === 'meta' && p.bd === boardId)
      if (prev) { Object.assign(prev.f, clone(op.f)); return pending }
    }
    pending.push(Object.assign(clone(op), { bd: boardId }))
    return pending
  }

  // An older stored object, read as format 1: list items without ids get them by position.
  function withIds(v) {
    if (!isObj(v)) return v
    const out = Object.assign({}, v)
    const fill = (list, p) => list.map((x, i) => (isObj(x) && x.id == null ? Object.assign({ id: p + '-' + i }, x) : x))
    if (Array.isArray(out.checks) && out.checks.some((x) => isObj(x) && x.id == null)) out.checks = fill(out.checks, 'ck')
    if (Array.isArray(out.replies) && out.replies.some((x) => isObj(x) && x.id == null)) out.replies = fill(out.replies, 'rp')
    if (out.type === 'table' && Array.isArray(out.rows) && out.rows.every(Array.isArray)) {
      if (!Array.isArray(out.rowIds) || out.rowIds.length !== out.rows.length) out.rowIds = out.rows.map((_, i) => 'rw-' + i)
      const n = out.rows.reduce((m, r) => Math.max(m, r.length), 0)
      if (!Array.isArray(out.colIds) || out.colIds.length !== n) out.colIds = Array.from({ length: n }, (_, i) => 'cl-' + i)
    }
    return out
  }

  function create() {
    const known = new Map()      // key → {rev, value}: the store as last seen here
    const regs = new Map()       // board id → {id, name, project, ...}
    const projs = new Map()      // project id → {id, name, parent, ...}
    const sent = new Set()       // write ids from here: their echoes are skipped
    let boardId = null
    let pending = []; let inflight = []; let timer = null; let writing = false; let ready = false
    let early = []               // edits made before the first board opened
    let holdFn = () => false; let replay = () => {}; let force = false; let lostError = ''; let failures = 0; let off = false
    const prefs = {}; let back = null
    let status = { state: 'pending', text: 'Loading…' }
    const listeners = []
    const setStatus = (state, text) => {
      status = { state, text: lostError && state !== 'saving' ? lostError : text, detail: 'Edits save on this computer, in Board’s K2 data' }
      listeners.forEach((f) => f(status))
    }
    const pfx = (id) => 'board/' + seg(id) + '/'
    const live = () => [...regs.values()].filter((r) => !r.trashedAt)
    const newBoardId = () => { let id; do id = 'b' + hex(3); while (regs.has(id) || known.has('boards/' + id)); return id }

    // A stored value → an object the model can show. A field that doesn't pass the schema isn't shown (it stays
    // stored as it is: writes merge into the stored value, so nothing is lost).
    function toObj(v) {
      if (!isObj(v) || typeof v.id !== 'string' || !S.TYPES[v.type]) return null
      const o = Object.assign({ id: v.id, type: v.type, x: 0, y: 0, z: 0 }, clone(S.DEFAULTS[v.type] || {}))
      for (const k of Object.keys(v)) {
        if (k === 'id' || k === 'type' || k === 'batch') continue
        if (OWN.includes(k)) { if (typeof v[k] === 'string') o[k] = v[k]; continue }
        if (S.isExtraKey(v.type, k)) { Object.assign(o, S.splitExtra(v.type, { [k]: v[k] }).extra); continue }   // a field this build doesn't know: kept as is
        let f = null
        try { f = S.clean(v.type, { [k]: v[k] }) } catch (e) { if (!S.isRefusal(e)) throw e }
        if (f) Object.assign(o, f)
      }
      for (const k of ['checks', 'crop', 'link']) if (o[k] === null) delete o[k]
      return S.ensureIds(o)
    }
    const metaOf = (v) => { const f = {}; if (isObj(v)) for (const k of Object.keys(v)) if (k !== 'batch') f[k] = v[k]; return f }
    const fieldsOf = (o) => { const f = {}; for (const k of Object.keys(o)) if (k !== 'id' && k !== 'type' && !OWN.includes(k)) f[k] = o[k]; return f }

    // The ops that turn the object stored as pv into the one stored as v.
    function feedOps(id, pv, v) {
      if (!v) return pv ? [{ op: 'del', id }] : []
      const o = toObj(v); if (!o || o.id !== id) return []
      const a = pv && toObj(pv)
      if (!a) return [{ op: 'add', obj: o }, { op: 'set', id, f: fieldsOf(o) }]   // the set covers an object the page already had
      const f = {}; const fo = fieldsOf(o)
      for (const k of Object.keys(fo)) if (JSON.stringify(fo[k]) !== JSON.stringify(a[k])) f[k] = fo[k]
      // A field removed elsewhere (one we don't know, or one with no default: checks, crop, link): gone here too.
      for (const k of Object.keys(fieldsOf(a))) if (!(k in fo) && a[k] != null) f[k] = null
      return Object.keys(f).length ? [{ op: 'set', id, f }] : []
    }

    // An edit made here and not yet stored wins over the stored one, for the fields it changes.
    function guard(op) {
      const id = op.op === 'add' ? op.obj.id : op.id
      for (const p of inflight.concat(pending)) {
        if (p.bd !== boardId) continue
        if (op.op === 'meta') {
          if (p.op !== 'meta') continue
          const f = Object.assign({}, op.f); for (const k of Object.keys(p.f)) delete f[k]
          if (!Object.keys(f).length) return null
          op = { op: 'meta', f }; continue
        }
        const pid = p.op === 'add' ? p.obj.id : p.id
        if (pid !== id) continue
        if (p.op === 'del') return op.op === 'del' ? op : null
        if (p.op === 'add') return null
        if (p.op === 'set' && op.op === 'set') {
          const f = Object.assign({}, op.f); for (const k of Object.keys(p.f)) delete f[k]
          if (!Object.keys(f).length) return null
          op = { op: 'set', id, f }
        }
      }
      return op
    }

    // A project's chain up to the top (itself first), and its whole subtree (itself included). Loops are cut:
    // a loop (two moves at once) or a missing parent counts as the top level.
    function chain(id) {
      const out = []; let q = id
      while (q && out.length < 64) { if (out.includes(q)) return [id]; out.push(q); const x = projs.get(q); q = x && x.parent; if (q && !projs.has(q)) break }
      return out
    }
    function subtree(id) { const out = new Set([id]); let grew = true; while (grew) { grew = false; for (const x of projs.values()) if (x.parent && out.has(x.parent) && !out.has(x.id) && chain(x.id).includes(id)) { out.add(x.id); grew = true } } return out }
    const strOr = (v, d) => (typeof v === 'string' && v ? v : d)
    function setProj(id, v) {
      if (isObj(v)) projs.set(id, { id, name: typeof v.name === 'string' && v.name.trim() ? v.name.trim() : 'Untitled project', trashedAt: strOr(v.trashedAt, null), trashedWith: strOr(v.trashedWith, null), parent: strOr(v.parent, null), order: typeof v.order === 'number' ? v.order : null })
      else projs.delete(id)
    }
    function setReg(id, v) {
      if (isObj(v)) regs.set(id, { id, name: strOr(v.name, 'Board'), project: strOr(v.project, null), agentEdit: typeof v.agentEdit === 'string' && v.agentEdit ? v.agentEdit : false, trashedAt: strOr(v.trashedAt, null), trashedWith: strOr(v.trashedWith, null), order: typeof v.order === 'number' ? v.order : null })
      else regs.delete(id)
    }

    // One key changed (prev = what the page had): record it, and queue ops for the open board in out (author → ops).
    function take(key, rev, v, by, prev, out) {
      if (v) known.set(key, { rev, value: v }); else known.delete(key)
      let m = REG_RE.exec(key)
      if (m) { setReg(m[1], v); return }
      if ((m = PROJ_RE.exec(key))) { setProj(m[1], v); return }
      if (!boardId || !(m = KEY_RE.exec(key)) || key.indexOf(pfx(boardId)) !== 0) return   // another board: read when it opens
      const who = personOf(by) || (isObj(v) && typeof v.updatedBy === 'string' && v.updatedBy) || 'someone'
      const ops = m[3] ? [{ op: 'meta', f: metaOf(v) }] : feedOps(m[2], prev && prev.value, v)
      for (let op of ops) { op = guard(op); if (op) { if (!out.has(who)) out.set(who, []); out.get(who).push(op) } }
    }
    function play(out) {
      const at = new Date().toISOString()
      for (const [by, ops] of out) if (ops.length) replay({ b: rid('k'), board: boardId, by, at, ops })
    }

    function onFeed(d, prefix) {
      if (!ready) return
      if (d && d.resync) return resync(Array.isArray(d.items) ? d.items : [], null, prefix)
      if (!Array.isArray(d)) return
      const out = new Map()
      for (const it of d) {
        if (!it || typeof it.key !== 'string' || typeof it.rev !== 'number') continue
        const prev = known.get(it.key)
        if (prev && prev.rev >= it.rev) continue                   // seen already: our own write, answered first
        const v = it.value == null || it.deleted ? null : it.value
        // Our own write's echo (an agent's raw put may copy our write id, so its `by` decides).
        if (isObj(v) && sent.has(v.batch) && !(typeof it.by === 'string' && it.by.indexOf('agent:') === 0)) { known.set(it.key, { rev: it.rev, value: v }); continue }
        take(it.key, it.rev, v, it.by, prev, out)
      }
      play(out)
      afterChange()
    }

    // The whole state under prefix again (after a gap). old: what the page shows now (key → {value}).
    function resync(items, old, prefix) {
      const prevKnown = new Map()
      for (const [k, e] of known) if (k.indexOf(prefix) === 0) { prevKnown.set(k, e); known.delete(k) }
      if ('boards/'.indexOf(prefix) === 0 || prefix === 'boards/') regs.clear()
      if ('projects/'.indexOf(prefix) === 0 || prefix === 'projects/') projs.clear()
      const was = old || prevKnown
      const out = new Map(); const seen = new Set()
      for (const it of items) {
        if (!it || typeof it.key !== 'string') continue
        seen.add(it.key)
        take(it.key, it.rev, it.value == null ? null : it.value, it.by, was.get(it.key), out)
      }
      for (const [k, prev] of was) if (!seen.has(k) && k.indexOf(prefix) === 0) take(k, 0, null, null, prev, out)
      play(out)
      afterChange()
    }
    // Any board list change: the last live board gone (an agent deleted it) leaves the person a new one.
    let afterChange = () => {}

    // The writes for some pending ops, as store ops with ifRev from what this page last saw.
    function plan(ops, b, at) {
      const next = new Map()
      const val = (k) => (next.has(k) ? next.get(k) : known.has(k) ? known.get(k).value : null)
      for (const op of ops) {
        const p = pfx(op.bd)
        if (op.op === 'add') {
          const k = p + 'obj/' + seg(op.obj.id); const v0 = isObj(val(k)) ? val(k) : {}
          const made = isObj(op.made) && typeof op.made.by === 'string' ? op.made : null   // an undo puts back the original author
          next.set(k, Object.assign({}, v0, op.obj, { createdBy: v0.createdBy || (made ? made.by : ME), createdAt: v0.createdAt || (made && typeof made.at === 'string' ? made.at : at), updatedBy: ME, updatedAt: at, batch: b }))
        } else if (op.op === 'set') {
          const k = p + 'obj/' + seg(op.id); const v0 = val(k)
          if (isObj(v0)) {   // deleted meanwhile: it stays deleted
            const v1 = withIds(Object.assign({}, v0, op.f, { updatedBy: ME, updatedAt: at, batch: b }))
            for (const f of Object.keys(op.f || {})) if (op.f[f] === null && S.dropsOnNull(v0.type, f)) delete v1[f]   // taken away (link, checks, crop, or unknown): no key, not null
            next.set(k, v1)
          }
        } else if (op.op === 'del') {
          const k = p + 'obj/' + seg(op.id)
          if (known.has(k)) next.set(k, null); else next.delete(k)
        } else if (op.op === 'meta') {
          const k = p + 'meta'; const v0 = isObj(val(k)) ? val(k) : {}
          next.set(k, Object.assign({ format: S.FORMAT }, v0, op.f, { batch: b }))
        } else if (op.op === 'reg') {
          const k = 'boards/' + seg(op.id); const v0 = isObj(val(k)) ? val(k) : {}
          next.set(k, Object.assign({ format: S.FORMAT }, v0, op.f))
        } else if (op.op === 'proj') {
          const k = 'projects/' + seg(op.id); const v0 = isObj(val(k)) ? val(k) : {}
          next.set(k, Object.assign({}, v0, op.f))
        }
      }
      const out = []; const big = []
      for (const [k, v] of next) {
        const e = known.get(k); const ifRev = e ? e.rev : 0
        if (v === null) { out.push({ op: 'delete', key: k, ifRev }); continue }
        if (S.bytesOf(v) > S.VALUE_MAX) { big.push(k); continue }
        out.push({ op: 'put', key: k, value: v, ifRev })
      }
      // The model refuses edits that make an object too big, so this is only a backstop: said, never silent.
      if (big.length) lostError = 'Not saved: ' + big.length + (big.length === 1 ? ' item is' : ' items are') + ' over 60 KB. Undo the last change.'
      return out
    }

    async function commit(w) {
      if (w.length <= BATCH_MAX) return g.k2.store.batch(w)
      const tx = g.k2.store.tx()
      try { for (let i = 0; i < w.length; i += BATCH_MAX) await tx.add(w.slice(i, i + BATCH_MAX)); return await tx.commit() } catch (e) { tx.abort().catch(() => {}); throw e }
    }

    // Write ops as one rev. On a conflict: take the newer values (shown, minus our fields), then re-apply ours; 3 tries.
    async function write(ops) {
      const b = rid('w'); const at = new Date().toISOString(); sent.add(b)
      for (let attempt = 0; ; attempt++) {
        const w = plan(ops, b, at)
        if (!w.length) return
        try {
          const r = await commit(w)
          const rev = r && typeof r.rev === 'number' ? r.rev : 0
          for (const x of w) { if (x.op === 'delete') known.delete(x.key); else known.set(x.key, { rev, value: x.value }) }
          return
        } catch (e) {
          if (codeOf(e) !== 'conflict' || attempt >= 2) throw e
          const out = new Map()
          for (const x of w) {
            const now = await g.k2.store.get(x.key); const prev = known.get(x.key)
            if (now && prev && prev.rev >= now.rev) continue
            take(x.key, now ? now.rev : 0, now ? now.value : null, now && now.by, prev, out)
          }
          play(out)
        }
      }
    }

    function schedule(now) {
      if (status.state !== 'error' && status.state !== 'saving') setStatus('pending', 'Saving…')
      clearTimeout(timer); timer = setTimeout(flush, now ? 0 : IDLE_MS)
    }

    // Codes that a retry can't fix: the next edit (or turning Board on) tries again.
    const STOP = ['core_off', 'not_granted', 'author_unknown', 'stale_widget', 'value_too_large', 'space_full', 'store_full']
    async function flush() {
      if (!pending.length) return
      if (writing || !ready || (!force && holdFn())) { clearTimeout(timer); timer = setTimeout(flush, IDLE_MS); return }
      writing = true
      inflight = pending.splice(0, TX_MAX)
      setStatus('saving', 'Saving…')
      let wait = 0; let stop = false
      try {
        await write(inflight)
        failures = 0; off = false
        setStatus(pending.length ? 'pending' : 'saved', pending.length ? 'Saving…' : 'Saved')
      } catch (e) {
        const code = codeOf(e)
        let again = []; for (const op of inflight.concat(pending)) again = mergeOp(again, op, op.bd)
        pending = again; failures++
        stop = STOP.includes(code); off = code === 'core_off'
        wait = code === 'rate_limited' ? Math.max(1, +(e && e.retryAfterSecs) || 2) * 1000 : Math.min(30000, 1000 * 2 ** failures)
        const say = {
          core_off: OFF_TEXT,
          conflict: 'Someone else keeps changing these: retrying',
          not_granted: 'Not saving: K2 doesn’t let this widget write Board’s data',
          author_unknown: 'Not saving: K2 doesn’t recognise this version of Board',
          stale_widget: 'Not saving: Board was updated. Reload to keep saving',
          paused: 'Saving is paused in K2',
          space_full: 'Not saving: Board’s data is full',
          store_full: 'Not saving: K2’s data store is full',
          value_too_large: 'Not saved: an item is over 60 KB. Undo the last change.',
        }
        setStatus('error', say[code] || 'Not saved (' + code + '): retrying')
      }
      inflight = []; writing = false
      if (pending.length && !stop) { clearTimeout(timer); timer = setTimeout(flush, force ? 0 : Math.max(wait, IDLE_MS)) }
    }

    // Wait until these ops are stored (other edits may still be waiting): {saved, why}.
    async function settle(ops) {
      schedule(true)
      const waiting = () => ops.some((o) => pending.includes(o) || inflight.includes(o) || pending.some((x) => x.op === o.op && x.id === o.id) || inflight.some((x) => x.op === o.op && x.id === o.id))
      for (let i = 0; i < 150 && waiting() && !off; i++) await later(100)
      return waiting() ? { saved: false, why: status.state === 'error' ? status.text : '' } : { saved: true }
    }

    function savePrefs() { g.k2.save.set('prefs', prefs).catch(() => {}) }

    // subscribe(prefix): resolves with the first delivery (the whole state under it); later ones go to onFeed.
    function follow(prefix) {
      return new Promise((resolve, reject) => {
        let first = true
        const t = setTimeout(() => { if (first) { first = false; reject(Object.assign(new Error('timeout'), { code: 'timeout' })) } }, 15000)
        g.k2.store.subscribe(prefix, (d) => {
          if (!first) return onFeed(d, prefix)
          first = false; clearTimeout(t); resolve(Array.isArray(d) ? d : (d && d.items) || [])
        }, (e) => {
          if (first) { first = false; clearTimeout(t); return reject(e) }
          setStatus('error', codeOf(e) === 'core_off' ? OFF_TEXT : 'Live updates stopped (' + codeOf(e) + '): reload to see others’ edits')
        })
      })
    }

    // A fresh box: the first board, made once even when two windows start together. The `seeded` key is
    // written with ifRev in the same batch, so only one window's board lands; the other reads it back.
    async function seed() {
      for (let attempt = 0; attempt < 3 && !live().length; attempt++) {
        const s = known.get('seeded'); const id = newBoardId(); const at = new Date().toISOString()
        const row = { id, name: 'Board', project: null, format: S.FORMAT }
        const w = [
          { op: 'put', key: 'seeded', value: Object.assign({}, s && isObj(s.value) ? s.value : {}, { at }), ifRev: s ? s.rev : 0 },
          { op: 'put', key: 'boards/' + id, value: row, ifRev: 0 },
          { op: 'put', key: pfx(id) + 'meta', value: { format: S.FORMAT, title: 'Board' }, ifRev: 0 },
        ]
        try {
          const r = await g.k2.store.batch(w)
          const rev = r && typeof r.rev === 'number' ? r.rev : 0
          for (const x of w) known.set(x.key, { rev, value: x.value })
          setReg(id, row)
          return id
        } catch (e) {
          if (codeOf(e) !== 'conflict') throw e
          await reread()
        }
      }
      return null
    }
    // Read the board list (and the seed mark) straight from the store, not waiting for the live feed.
    async function reread() {
      const s = await g.k2.store.get('seeded')
      if (s) known.set('seeded', { rev: s.rev, value: s.value })
      let after
      for (let n = 0; n < 100; n++) {
        const page = await g.k2.store.list(after ? { prefix: 'boards/', after } : { prefix: 'boards/' })
        for (const it of (page && page.items) || []) { const prev = known.get(it.key); if (!prev || prev.rev < it.rev) take(it.key, it.rev, it.value, it.by, prev, new Map()) }
        if (!page || !page.next) break
        after = page.next
      }
    }
    const byOrder = (a, b) => (a.order ?? 1e9) - (b.order ?? 1e9) || (a.name || '').localeCompare(b.name || '') || (a.id < b.id ? -1 : 1)

    // A new board's rows: the list row and its meta, both format 1.
    function makeBoard(name, project) {
      const id = newBoardId()
      const row = { id, name: name || 'Untitled board', project: project || null, format: S.FORMAT }
      regs.set(id, Object.assign({ agentEdit: false, trashedAt: null, trashedWith: null, order: null }, row))
      pending.push({ op: 'reg', id, f: row, bd: id })
      pending = mergeOp(pending, { op: 'meta', f: { format: S.FORMAT, title: row.name } }, id)
      return id
    }

    // What changed on the open board since a time (ms): K2's change feed, deletions included.
    async function history(since) {
      const out = []; const prefix = pfx(boardId) + 'obj/'; let from = 0; let full = true
      for (let n = 0; ; n++) {
        if (n === 40) { full = false; break }   // ~6,000 changes: enough for a day; never a runaway loop
        const page = await g.k2.store.changes({ since: from, prefix })
        for (const e of (page && page.items) || []) {
          const at = (e.at || 0) * 1000; if (!(at >= since) || !KEY_RE.exec(e.key)) continue
          const v = known.get(e.key)
          out.push({ id: v && v.value && v.value.id ? v.value.id : e.key.slice(prefix.length), at, by: personOf(e.by) || e.by, deleted: !!e.deleted })
        }
        const next = page && page.next && Number(page.next.since)
        if (!(next > from)) break; from = next
      }
      out.sort((a, b) => b.at - a.at); out.partial = !full
      return out
    }

    const api = {
      rid, history,
      me: ME, isMe, isAgent, nameOf, samePerson,
      get viewerName() { return viewer || 'You' },
      load: () => ({ boardId: null, board: { objects: {}, meta: {} } }),
      setHold(fn) { holdFn = fn },
      async start(applyRemote) {
        replay = applyRemote
        try {
          await g.k2.connected
          await loadPeople()
          let saved = null
          try { saved = await g.k2.save.get('prefs') } catch (_) {}
          if (isObj(saved)) Object.assign(prefs, saved)
          const [reg, objs, prj] = await Promise.all([follow('boards/'), follow('board/'), follow('projects/')])
          const s = await g.k2.store.get('seeded')
          if (s) known.set('seeded', { rev: s.rev, value: s.value })
          ready = true
          resync(reg.concat(objs, prj), new Map(), '')
          if (!live().length) await seed()
          if (!live().length) throw Object.assign(new Error('no board'), { code: 'no_board' })
          const open = typeof prefs.open === 'string' && regs.has(prefs.open) && !regs.get(prefs.open).trashedAt ? prefs.open : live().sort(byOrder)[0].id
          back = { board: open }
          if (typeof prefs.scheme === 'string') back.scheme = prefs.scheme
          afterChange = () => { if (ready && !live().length && !writing) { makeBoard('Board', null); schedule() } }
          setStatus(pending.length ? 'pending' : 'saved', pending.length ? 'Saving…' : 'Saved')
        } catch (e) {
          ready = true
          off = codeOf(e) === 'core_off'
          setStatus('error', off ? OFF_TEXT : 'Couldn’t load your boards from K2 (' + codeOf(e) + ')')
          throw e
        }
        if (pending.length) schedule()
      },
      submit(ops) {
        if (!boardId) { early.push(...ops); return }
        for (const op of ops) pending = mergeOp(pending, op, boardId)
        if (pending.length) schedule()
      },
      boards: () => Array.from(regs.values(), (b) => Object.assign({}, b)),
      get boardId() { return boardId },
      open(id) {
        if (id === boardId || !regs.has(id)) return null
        const first = !boardId
        boardId = id; prefs.open = id; savePrefs()
        if (first && early.length) { for (const op of early) pending = mergeOp(pending, op, id); early = []; schedule() }
        // Edits to that board made here and not stored yet still show.
        const ops = inflight.concat(pending).filter((p) => p.bd === id).map((p) => { const c = clone(p); delete c.bd; return c })
        const p = pfx(id); const objects = {}; let meta = {}
        for (const [k, e] of known) {
          if (k.indexOf(p) !== 0) continue
          if (k === p + 'meta') { meta = metaOf(e.value); continue }
          const o = toObj(e.value); if (o && k === p + 'obj/' + seg(o.id)) objects[o.id] = o
        }
        return { board: { objects, meta }, later: ops.length ? [{ b: rid('w'), board: id, by: ME, at: new Date().toISOString(), ops }] : [] }
      },
      // project: the folder it's made in (undefined: the open board's project).
      create(name, project) {
        const cur = regs.get(boardId)
        const id = makeBoard(name, project !== undefined ? (project && projs.has(project) ? project : null) : cur ? cur.project : null)
        schedule()
        return id
      },
      // A board that isn't open: its own title changes too (the open board's goes through the model, undoable).
      rename(id, name) {
        const r = regs.get(id); if (!r) return
        r.name = name
        const p = pending.find((x) => x.op === 'reg' && x.id === id)
        if (p) p.f.name = name; else pending.push({ op: 'reg', id, f: { name }, bd: id })
        if (id !== boardId) pending = mergeOp(pending, { op: 'meta', f: { title: name } }, id)
        schedule()
      },
      prefs: () => Object.assign({}, prefs),
      setPref(k, v) { prefs[k] = v; savePrefs() },
      // ---------- the Trash: deleting marks a row (trashedAt); the data stays, restorable ----------
      // A project's boards go with it (trashedWith: the project) and come back with it. When the last live
      // board goes, a new empty one is made, so there's always a board to show.
      async trash(kind, id) {
        const at = new Date().toISOString(); const mark = pending.length
        if (kind === 'project') {
          const p = projs.get(id); if (!p) return { saved: false }
          const tree = subtree(id)
          p.trashedAt = at; p.trashedWith = null; pending.push({ op: 'proj', id, f: { trashedAt: at, trashedWith: null, trashedBy: ME }, bd: boardId })
          for (const q of tree) if (q !== id) { const x = projs.get(q); if (x && !x.trashedAt) { x.trashedAt = at; x.trashedWith = id; pending.push({ op: 'proj', id: q, f: { trashedAt: at, trashedWith: id, trashedBy: ME }, bd: boardId }) } }
          for (const r of regs.values()) if (tree.has(r.project) && !r.trashedAt) { r.trashedAt = at; r.trashedWith = id; pending.push({ op: 'reg', id: r.id, f: { trashedAt: at, trashedWith: id, trashedBy: ME }, bd: r.id }) }
        } else {
          const r = regs.get(id); if (!r) return { saved: false }
          r.trashedAt = at; r.trashedWith = null; pending.push({ op: 'reg', id, f: { trashedAt: at, trashedWith: null, trashedBy: ME }, bd: id })
        }
        const created = live().length ? null : makeBoard('Board', null)
        const r = await settle(pending.slice(mark))
        return Object.assign(r, created ? { created } : {})
      },
      async restore(kind, id) {
        const mark = pending.length
        if (kind === 'project') {
          const p = projs.get(id); if (!p) return { saved: false }
          // Its parent (or one above) still in the Trash: it comes back at the top level.
          const up = chain(id).slice(1); const pf = { trashedAt: null, trashedWith: null, trashedBy: null }
          if (up.some((q) => projs.get(q) && projs.get(q).trashedAt)) { pf.parent = null; p.parent = null }
          p.trashedAt = null; pending.push({ op: 'proj', id, f: pf, bd: boardId })
          for (const x of projs.values()) if (x.trashedWith === id) { x.trashedAt = null; x.trashedWith = null; pending.push({ op: 'proj', id: x.id, f: { trashedAt: null, trashedWith: null, trashedBy: null }, bd: boardId }) }
          for (const r of regs.values()) if (r.trashedWith === id) { r.trashedAt = null; r.trashedWith = null; pending.push({ op: 'reg', id: r.id, f: { trashedAt: null, trashedWith: null, trashedBy: null }, bd: r.id }) }
        } else {
          const r = regs.get(id); if (!r) return { saved: false }
          const f = { trashedAt: null, trashedWith: null, trashedBy: null }
          if (r.project && (!projs.has(r.project) || chain(r.project).some((q) => projs.get(q) && projs.get(q).trashedAt))) { f.project = null; r.project = null }   // its project is in the Trash: it comes back under "No project"
          r.trashedAt = null; r.trashedWith = null; pending.push({ op: 'reg', id, f, bd: id })
        }
        return settle(pending.slice(mark))
      },
      // ---------- Board's own projects ----------
      projects: () => Array.from(projs.values(), (p) => Object.assign({}, p)),   // trashed ones too (trashedAt)
      createProject(name, parent) {
        let id; do id = 'p' + hex(3); while (projs.has(id))
        parent = parent && projs.has(parent) ? parent : null
        projs.set(id, { id, name, parent, order: null, trashedAt: null, trashedWith: null })
        pending.push({ op: 'proj', id, f: { id, name, parent }, bd: boardId }); schedule()
        return id
      },
      // Put a project inside another (or at the top level: null). Never inside itself or its own subtree.
      async nest(id, parent) {
        const p = projs.get(id); parent = parent || null
        if (!p || (parent && (!projs.has(parent) || subtree(id).has(parent)))) return { saved: false, refused: true }
        if ((p.parent || null) === parent) return { saved: true, unchanged: true }
        p.parent = parent; const mark = pending.length
        pending.push({ op: 'proj', id, f: { parent }, bd: boardId })
        return settle(pending.slice(mark))
      },
      // The order of rows in one folder: ids (boards, or projects) in the new order.
      reorder(kind, ids) {
        ids.forEach((x, i) => {
          const row = kind === 'project' ? projs.get(x) : regs.get(x); if (!row || row.order === i) return
          row.order = i; pending.push({ op: kind === 'project' ? 'proj' : 'reg', id: x, f: { order: i }, bd: kind === 'project' ? boardId : x })
        })
        schedule()
      },
      renameProject(id, name) {
        const p = projs.get(id); if (!p) return
        p.name = name
        const q = pending.find((x) => x.op === 'proj' && x.id === id)
        if (q) q.f.name = name; else pending.push({ op: 'proj', id, f: { name }, bd: boardId })
        schedule()
      },
      // Move a board to a project (or none): one write of its list row (its keys don't name the project).
      async move(id, to) {
        to = to || null
        const r = regs.get(id); if (!r) throw Object.assign(new Error('no such board'), { code: 'no_board' })
        if ((r.project || null) === to) return { unchanged: true }
        if (to && !projs.has(to)) throw Object.assign(new Error('no such project'), { code: 'no_project' })
        r.project = to; const mark = pending.length
        pending.push({ op: 'reg', id, f: { project: to }, bd: id })
        const s = await settle(pending.slice(mark))
        if (!s.saved) throw Object.assign(new Error('still saving'), { code: writing || pending.length ? 'busy' : 'not_saved' })
        return { objects: [...known.keys()].filter((k) => k.indexOf(pfx(id) + 'obj/') === 0).length }
      },
      // What to bring back after loading: the board to open and light/dark, from the person's private saves.
      restored() { const b = back; back = null; return b },
      flushNow() {
        if (!pending.length || !ready || writing) return
        clearTimeout(timer); force = true; flush().finally(() => { force = false })
      },
      onStatus(cb) { listeners.push(cb); cb(status) },
      // ---------- images: K2's blobs (the original bytes, never re-encoded) ----------
      canBlobs: () => may('blobs.put'),
      putBlob: (file, name) => g.k2.blobs.put(file, name ? { name: String(name).slice(0, 255) } : undefined),
      blobUrl: (ref, px) => g.k2.blobs.url(ref, px ? { px } : undefined),
      blobGet: (ref) => g.k2.blobs.get(ref),
      // ---------- the JSON export's source: the open board as stored (pending edits saved first) ----------
      async exportSource() {
        api.flushNow()
        for (let i = 0; i < 150 && (pending.some((p) => p.bd === boardId) || writing); i++) await later(100)
        if (pending.some((p) => p.bd === boardId)) throw Object.assign(new Error('not saved yet'), { code: 'not_saved' })
        const p = pfx(boardId); const objects = []; let meta = {}
        for (const [k, e] of known) {
          if (k.indexOf(p) !== 0) continue
          if (k === p + 'meta') { meta = clone(e.value); continue }
          if (k.indexOf(p + 'obj/') === 0 && isObj(e.value) && typeof e.value.id === 'string') objects.push(clone(e.value))
        }
        const reg = regs.get(boardId)
        return { meta, objects, name: reg ? reg.name : 'Board' }
      },
    }

    // ---------- where boards come from: the switcher lists every source's boards ----------
    // One source today: this Garden's own data. Later sources (board files in other workspaces) give rows with
    // `workspace` set ({id, label}); the switcher shows it as a badge. Each row:
    //   {id, title, workspace: null, project, order, trashed, trashedAt, trashedWith, agentEdit}
    api.sources = [{
      id: 'garden',
      list: () => Array.from(regs.values(), (b) => ({ id: b.id, title: b.name, workspace: null, project: b.project, order: b.order, trashed: !!b.trashedAt, trashedAt: b.trashedAt, trashedWith: b.trashedWith, agentEdit: b.agentEdit })),
      open: (id) => api.open(id),
    }]

    // ---------- the Garden's Chat column: K2's own Conversation, steered from the board ----------
    // The chip's text: the latest wanted, the last sent. A refusal is tried again at most twice, then not until
    // the next context() call. Sends stay 600 ms apart (K2 takes 2 a second).
    const chip = { want: null, sent: null, timer: null, fails: 0, at: 0 }
    function sendChip() {
      chip.timer = null
      if (chip.want === chip.sent || chip.fails >= 3) return
      const t = chip.want; chip.sent = t; chip.at = Date.now()
      const failed = () => { if (chip.sent !== t) return; chip.sent = null; chip.fails++; if (!chip.timer && chip.fails < 3) chip.timer = setTimeout(sendChip, 600 * 2 ** chip.fails) }
      g.k2.conversation.setContext(t).then(() => { if (chip.sent === t) chip.fails = 0 }, failed)
      if (!chip.timer) chip.timer = setTimeout(sendChip, 600)   // then the latest, if it changed meanwhile
    }
    const chatColumn = (v) => ((v && v.columns) || []).find((x) => x && x.collapsible && /chat/i.test(String(x.label || '')))
    // The agents that can work on a board: those with a data grant that covers board/<b>/ (K2's own grants,
    // store.grantees) → {agents, fromGrants: true}. Without that verb (or when K2 refuses it): every agent on
    // this computer, {agents, fromGrants: false}.
    api.boardAgents = async (id) => {
      const rows = await localAgents()
      if (!may('store.grantees')) return { agents: rows, fromGrants: false }
      let items
      try { const r = await g.k2.store.grantees(); items = r && Array.isArray(r.items) ? r.items : null } catch (_) { items = null }
      if (!items) return { agents: rows, fromGrants: false }
      const pre = pfx(id); const out = []
      for (const it of items) {
        if (!it || typeof it.handle !== 'string') continue
        const covers = it.prefixes == null || (Array.isArray(it.prefixes) && it.prefixes.some((q) => typeof q === 'string' && (pre.indexOf(q) === 0 || q.indexOf(pre) === 0)))
        const row = covers && rows.find((r) => r.handle === it.handle)
        if (row && !out.some((x) => x.handle === row.handle)) out.push(Object.assign({ level: it.level === 'write' ? 'write' : 'read' }, row))
      }
      return { agents: out, fromGrants: true }
    }
    api.chat = {
      localAgents,
      agents: async (id) => (await api.boardAgents(id)).agents,
      // The agent a board opens the chat with (a board file opens with its own workspace's agent): an address,
      // or null to leave the Conversation's pick as it is. This Garden's boards: null.
      defaultAgent: () => null,
      // Show that agent in the Conversation beside the board.
      openAgent: (address) => g.k2.conversation.open(address).then(() => true, () => false),
      // The Agents list beside the board shows just these, in this order.
      async show(addresses) { try { await g.k2.agents.show(addresses.slice(0, 50)); return true } catch (_) { return false } },
      // A chip on the Conversation's message box, sent with each of the person's messages: "Board: <name> · <id>",
      // one line of at most 120 characters (the name is cut, never the id). Not board data.
      context(name, id) {
        const flat = (v) => String(v).replace(/[\r\n\v\f\u0085\u2028\u2029]+/g, ' ').replace(/\[(thread:|from |context)/gi, '(').trim()
        const tail = ' · ' + Array.from(flat(id)).slice(0, 40).join(''); const n = Array.from(flat(name) || 'Board')
        while (n.length > 1 && ('Board: ' + n.join('') + tail).length > 120) n.pop()
        chip.want = 'Board: ' + n.join('') + tail; chip.fails = 0
        if (chip.want === chip.sent) return
        clearTimeout(chip.timer); chip.timer = setTimeout(sendChip, Math.max(0, chip.at + 600 - Date.now()))
      },
      // The Chat column, open or folded: cb(true | false | null when the page has none), now and on every fold.
      watchColumn(cb) {
        const pick = (v) => { const c = chatColumn(v); cb(c ? !c.collapsed : null) }
        g.k2.layout.get().then(pick, () => cb(null))
        const stop = g.k2.layout.subscribe(pick)
        return () => { if (typeof stop === 'function') stop() }
      },
      // "Ask an agent about this": show that agent in the Conversation, unfold the Chat column, and put the text in
      // its message box. Never sends: the person reads it, edits it and sends it. False when it couldn't.
      async draft(address, text) {
        const c = chatColumn(await g.k2.layout.get().catch(() => null))
        if (c && c.collapsed) await g.k2.layout.set({ column: c.index, collapsed: false }).catch(() => {})
        try { await g.k2.conversation.open(address); await g.k2.compose.draft(address, String(text).slice(0, 4000)); return true } catch (_) { return false }
      },
      // Open or fold the Garden's Chat column (right after the person's click; false when there's none).
      async toggle() {
        const c = chatColumn(await g.k2.layout.get().catch(() => null))
        if (!c) return false
        try { await g.k2.layout.toggle(c.index); return true } catch (_) { return false }
      },
    }
    return api
  }

  g.BoardStore = { create }
})(window)
