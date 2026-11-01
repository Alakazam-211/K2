// BoardModel: the board's objects and settings, ops, undo/redo and previews.
// Every change is an op ({op:'add'|'set'|'del'|'meta'}), validated by schema.js.
// A person's edit is all or nothing: an op the schema refuses (too long, too many, a bad value, an object over
// 60 KB) leaves the board exactly as it was and is reported to onRefuse listeners, never cut or skipped.
(function (g) {
  'use strict'
  const S = g.BoardSchema
  const BLANK = { rot: 0, locked: false, group: '', tags: [], reactions: {}, votes: {}, z: 0 }
  const clone = (v) => (v === undefined ? undefined : JSON.parse(JSON.stringify(v)))

  function create(snap, me) {
    const objects = clone(snap.objects || {})
    const meta = clone(snap.meta || {})
    const listeners = []; const opListeners = []; const refuseListeners = []
    let undoStack = []; let redoStack = []
    const preview = new Map()   // id -> original values of fields changed by a live preview

    const emit = (ids, metaChanged) => listeners.forEach((f) => f(ids, !!metaChanged))
    const refuse = (e) => refuseListeners.forEach((f) => f(e))
    const maxZ = () => { let z = 0; for (const o of Object.values(objects)) if (o.z > z) z = o.z; return z }
    const minZ = () => { let z = Infinity; for (const o of Object.values(objects)) if (o.z < z) z = o.z; return z === Infinity ? 0 : z }

    // Only schema fields travel in ops (no createdBy etc.), plus fields this build doesn't know (kept as they came).
    function strip(o) {
      const spec = Object.assign({}, S.COMMON, S.TYPES[o.type] || {})
      const out = { id: o.id, type: o.type }
      for (const k of Object.keys(o)) if ((spec[k] || S.isExtraKey(o.type, k)) && o[k] !== undefined) out[k] = clone(o[k])
      return out
    }
    const prevOf = (o, k) => (o[k] !== undefined ? clone(o[k]) : S.dropsOnNull(o.type, k) ? null   // had none: undo sends null (none), which survives the trip
      : (S.DEFAULTS[o.type] && S.DEFAULTS[o.type][k] !== undefined ? clone(S.DEFAULTS[o.type][k]) : clone(BLANK[k])))
    // An object as it would be stored (its fields, the authors and a write id): never over 60 KB.
    const sized = (o) => S.checkSize(Object.assign({}, o, { batch: 'w-0000000000' }))
    // Objects change in place: the UI may hold one between a click and the next draw.
    const replace = (o, n) => { for (const k of Object.keys(o)) delete o[k]; Object.assign(o, n) }

    // Apply one op; returns its inverse (or null when there was nothing to do). Throws a refusal.
    function one(op, by, at, check, ids, out) {
      if (op.op === 'add') {
        if (objects[op.obj.id]) return null   // already there (a replayed write)
        const t = op.obj.type
        if (!S.TYPES[t]) throw S.refusal('unknown_type', 'type')
        const { known, extra } = S.splitExtra(t, strip(op.obj))
        const o = S.ensureIds(Object.assign({ id: op.obj.id, type: t, x: 0, y: 0 }, clone(S.DEFAULTS[t]), { z: maxZ() + 1 },
          S.clean(t, S.normalize(t, known)), extra, { createdBy: op.obj.createdBy || (op.made && op.made.by) || by, createdAt: op.obj.createdAt || (op.made && op.made.at) || at, updatedBy: by, updatedAt: at }))
        for (const k of ['checks', 'crop', 'link']) if (o[k] === null) delete o[k]
        if (check) sized(o)
        objects[op.obj.id] = o; ids.add(op.obj.id)
        return { op: 'del', id: op.obj.id }
      }
      if (op.op === 'set') {
        const o = objects[op.id]; if (!o) return null   // deleted meanwhile
        const { known, extra } = S.splitExtra(o.type, op.f || {})
        const f = S.clean(o.type, S.normalize(o.type, known, o)); const back = {}
        for (const k of Object.keys(f)) back[k] = prevOf(o, k)
        for (const k of Object.keys(extra)) back[k] = o[k] !== undefined ? clone(o[k]) : null
        const n = Object.assign(clone(o), clone(f), { updatedBy: by, updatedAt: at })
        for (const [k, v] of Object.entries(extra)) { if (v === null) delete n[k]; else n[k] = v }   // a field we don't know: as it came
        for (const k of Object.keys(f)) if (f[k] === null && S.dropsOnNull(o.type, k)) delete n[k]   // null means none: no key
        // A person's table edit keeps its row ids in step (an agent's older-style write gets ids by position).
        if (check && n.type === 'table' && f.rows && !f.rowIds && Array.isArray(o.rowIds) && o.rowIds.length !== f.rows.length) throw S.refusal('bad_field', 'rowIds', null, 'one id per row')
        S.ensureIds(n)
        if (check) sized(n)
        replace(o, n); ids.add(op.id)
        return { op: 'set', id: op.id, f: back }
      }
      if (op.op === 'del') {
        const o = objects[op.id]; if (!o) return null
        delete objects[op.id]; ids.add(op.id)
        return { op: 'add', obj: Object.assign(strip(o), { createdBy: o.createdBy, createdAt: o.createdAt }) }
      }
      if (op.op === 'meta') {
        const f = S.cleanMeta(op.f || {}); const back = {}
        for (const k of Object.keys(f)) back[k] = meta[k] !== undefined ? clone(meta[k]) : clone(S.META_OFF[k] !== undefined ? S.META_OFF[k] : null)
        Object.assign(meta, clone(f)); out.metaChanged = true
        return { op: 'meta', f: back }
      }
      throw S.refusal('bad_field', 'op', null, 'add, set, del or meta')
    }

    // Apply ops; returns the inverse ops (to undo them). strict (a person's edit): all or nothing, the first
    // refusal is thrown with the board untouched. Not strict (remote changes): each refused op is reported.
    function applyRaw(ops, by, at, strict) {
      const inverse = []; const ids = new Set(); const out = { metaChanged: false }
      const before = new Map(); const metaBefore = clone(meta)
      for (const op of ops) {
        const id = op.op === 'add' ? op.obj && op.obj.id : op.id
        if (strict && id != null && !before.has(id)) before.set(id, objects[id] ? clone(objects[id]) : undefined)
        try {
          const inv = one(op, by, at, strict, ids, out)
          if (inv) inverse.unshift(inv)
        } catch (e) {
          if (!S.isRefusal(e)) throw e
          if (strict) {
            for (const [k, v] of before) { if (v === undefined) delete objects[k]; else if (objects[k]) replace(objects[k], v); else objects[k] = v }
            for (const k of Object.keys(meta)) delete meta[k]
            Object.assign(meta, metaBefore)
            e.op = op; throw e
          }
          e.op = op; e.remote = true; refuse(e)
        }
      }
      return { inverse, ids, metaChanged: out.metaChanged }
    }
    const done = (r) => { emit(r.ids, r.metaChanged); return r.inverse }

    // Ops travel without createdBy/createdAt. An add carries the object as made here (z and defaults filled in),
    // so every copy of the board matches. An add that puts back something deleted (undo) says who made it and
    // when, beside the object (op.made), so the store keeps the original author instead of stamping "me, now".
    const wire = (ops) => ops.map((op) => {
      if (op.op !== 'add') return clone(op)
      const o = objects[op.obj.id] ? strip(objects[op.obj.id]) : clone(op.obj); delete o.createdBy; delete o.createdAt
      return op.obj.createdBy ? { op: 'add', obj: o, made: { by: op.obj.createdBy, at: op.obj.createdAt } } : { op: 'add', obj: o }
    })

    function send(ops) { const w = wire(ops); if (w.length) opListeners.forEach((f) => f(w)) }
    // A person's edit, all or nothing: false (and onRefuse hears why) when the schema refuses it.
    function attempt(ops) {
      try { return applyRaw(ops, me, new Date().toISOString(), true) } catch (e) { if (!S.isRefusal(e)) throw e; refuse(e); return null }
    }

    const api = {
      objects, meta, me,
      // People: the store sets these once it knows K2's names (who is "me", what to call a person key).
      isMe: (k) => k === me,
      nameOf: (k) => String(k || ''),
      get(id) { return objects[id] },
      all() { return Object.values(objects) },
      maxZ, minZ, strip,
      onChange(f) { listeners.push(f) },
      onOps(f) { opListeners.push(f) },
      onRefuse(f) { refuseListeners.push(f) },
      // A user action: applied, saved, undoable. Returns false when refused (nothing changed).
      apply(ops, opts) {
        if (!ops.length) return true
        api.cancelPreview()
        const r = attempt(ops); if (!r) return false
        send(ops)
        if (!(opts && opts.noUndo)) { undoStack.push({ undo: r.inverse, redo: clone(ops) }); if (undoStack.length > 200) undoStack.shift(); redoStack = [] }
        done(r)
        return true
      },
      // Edits from elsewhere (an agent, another window): applied, not saved again, not undoable.
      applyRemote(batch) { done(applyRaw(batch.ops || [], batch.by || 'someone', batch.at || new Date().toISOString(), false)) },
      undo() { const e = undoStack.pop(); if (!e) return false; api.cancelPreview(); const r = attempt(e.undo); if (!r) { undoStack.push(e); return false } send(e.undo); redoStack.push(e); done(r); return true },
      redo() { const e = redoStack.pop(); if (!e) return false; api.cancelPreview(); const r = attempt(e.redo); if (!r) { redoStack.push(e); return false } send(e.redo); undoStack.push(e); done(r); return true },
      canUndo: () => undoStack.length > 0,
      canRedo: () => redoStack.length > 0,
      // Live preview while dragging: changes objects, remembers originals, saves nothing.
      preview(id, f) {
        const o = objects[id]; if (!o) return
        if (!preview.has(id)) preview.set(id, {})
        const orig = preview.get(id)
        for (const k of Object.keys(f)) if (!(k in orig)) orig[k] = prevOf(o, k)
        Object.assign(o, f)
        emit(new Set([id]), false)
      },
      // Turn the preview into one undoable action.
      commitPreview() {
        if (!preview.size) return
        const ops = []; const ids = new Set()
        for (const [id, orig] of preview) {
          const o = objects[id]; if (!o) continue
          const f = {}; for (const k of Object.keys(orig)) f[k] = clone(o[k])
          Object.assign(o, clone(orig))   // back to the start, so apply() records the true inverse
          ops.push({ op: 'set', id, f }); ids.add(id)
        }
        preview.clear()
        if (!api.apply(ops)) emit(ids, false)   // refused: the objects are back where they were
      },
      cancelPreview() {
        if (!preview.size) return
        const ids = new Set()
        for (const [id, orig] of preview) { if (objects[id]) Object.assign(objects[id], clone(orig)); ids.add(id) }
        preview.clear(); emit(ids, false)
      },
      isPreviewing: () => preview.size > 0,
      // Another board: replace everything in place (the UI holds this same object), with fresh undo history.
      reset(next) {
        const ids = new Set(Object.keys(objects))
        preview.clear(); undoStack = []; redoStack = []
        for (const k of Object.keys(objects)) delete objects[k]
        for (const k of Object.keys(meta)) delete meta[k]
        Object.assign(objects, clone(next.objects || {})); Object.assign(meta, clone(next.meta || {}))
        for (const k of Object.keys(objects)) ids.add(k)
        emit(ids, true)
      },
      // The board's current contents.
      snapshot: () => ({ objects: clone(objects), meta: clone(meta) }),
    }
    return api
  }
  g.BoardModel = { create }
})(window)
