// BoardApp core: camera, selection, tools, pointer and keyboard, editing,
// clipboard and actions. The toolbars and panels live in ui.js.
(function (g) {
  'use strict'
  const S = g.BoardSchema; const G = g.BoardGeom
  const $ = (id) => document.getElementById(id)
  const viewport = $('viewport'); const world = $('world'); const overlay = $('overlay')

  const store = g.BoardStore.create()
  const loaded = store.load()
  const model = g.BoardModel.create(loaded.board, store.me)
  const R = g.BoardRender.create(world, model)
  model.onOps((ops) => store.submit(ops))
  store.onReadOnly((on) => model.setReadOnly(on))

  const listeners = {}
  const on = (ev, f) => { (listeners[ev] = listeners[ev] || []).push(f) }
  const fire = (ev, ...a) => (listeners[ev] || []).forEach((f) => f(...a))

  const App = {
    S, G, R, model, store, loaded, on, fire,
    cam: { x: 0, y: 0, s: 1 },
    sel: new Set(),
    tool: 'select', toolOpts: {},
    snap: false, reduced: false,
    editing: null,
    presenting: false,
    hover: null,
  }
  g.BoardApp = App

  // ---------- camera ----------
  const rect = () => viewport.getBoundingClientRect()
  App.toWorld = (sx, sy) => { const r = rect(); return [(sx - r.left - App.cam.x) / App.cam.s, (sy - r.top - App.cam.y) / App.cam.s] }
  App.toScreen = (wx, wy) => { const r = rect(); return [wx * App.cam.s + App.cam.x + r.left, wy * App.cam.s + App.cam.y + r.top] }
  App.viewBox = () => { const r = rect(); return { x: -App.cam.x / App.cam.s, y: -App.cam.y / App.cam.s, w: r.width / App.cam.s, h: r.height / App.cam.s } }
  let camRaf = 0
  // Level of detail: soft (blurred) sticky shadows cost a lot to paint; on a big board (over 600 stickies) or
  // zoomed out below 45%, where the blur can't be seen, they become crisp ones (measured: test/perf.js).
  let stickyCount = 0; let lastTotal = -1
  const lod = () => world.classList.toggle('lod', stickyCount > 600 || App.cam.s < 0.45)
  App.countStickies = () => { stickyCount = 0; for (const o of model.all()) if (o.type === 'sticky') stickyCount++; lod() }
  function applyCam() {
    const c = App.cam
    world.style.transform = 'translate(' + c.x + 'px,' + c.y + 'px) scale(' + c.s + ')'
    world.style.setProperty('--inv', String(1 / c.s))
    world.style.setProperty('--s', String(c.s))
    const step = 24 * c.s; const show = step >= 9
    viewport.style.backgroundSize = show ? step + 'px ' + step + 'px' : '0 0'
    viewport.style.backgroundPosition = c.x + 'px ' + c.y + 'px'
    viewport.classList.toggle('far', c.s < 0.35)
    lod()
    if (!camRaf) camRaf = requestAnimationFrame(() => { camRaf = 0; drawOverlay(); fire('camera') })
  }
  App.applyCam = applyCam
  App.zoomAt = (sx, sy, s) => {
    const r = rect(); const px = sx - r.left; const py = sy - r.top
    s = Math.min(8, Math.max(0.05, s))
    App.cam.x = px - (px - App.cam.x) * (s / App.cam.s); App.cam.y = py - (py - App.cam.y) * (s / App.cam.s); App.cam.s = s
    applyCam()
  }
  let anim = 0
  App.animateTo = (target, ms) => {
    cancelAnimationFrame(anim)
    if (App.reduced || !ms) { Object.assign(App.cam, target); applyCam(); return }
    const from = Object.assign({}, App.cam); const t0 = performance.now()
    // Interpolate scale geometrically so zooming feels even.
    const step = (now) => {
      const t = Math.min(1, (now - t0) / ms); const e = 1 - Math.pow(1 - t, 3)
      const s = Math.exp(Math.log(from.s) + (Math.log(target.s) - Math.log(from.s)) * e)
      // Keep the screen point that maps to the target centre moving smoothly.
      App.cam.s = s
      App.cam.x = from.x + (target.x - from.x) * e
      App.cam.y = from.y + (target.y - from.y) * e
      applyCam()
      if (t < 1) anim = requestAnimationFrame(step)
    }
    anim = requestAnimationFrame(step)
  }
  App.zoomCentre = (factor) => {
    const r = rect(); const s = Math.min(8, Math.max(0.05, App.cam.s * factor))
    const px = r.width / 2; const py = r.height / 2
    App.animateTo({ s, x: px - (px - App.cam.x) * (s / App.cam.s), y: py - (py - App.cam.y) * (s / App.cam.s) }, 180)
  }
  App.fitTo = (b, opts) => {
    if (!b) return
    const r = rect(); const pad = (opts && opts.pad) != null ? opts.pad : 80
    const top = (opts && opts.top) != null ? opts.top : 60
    const availW = Math.max(100, r.width - pad * 2); const availH = Math.max(100, r.height - pad * 2 - top)
    const s = Math.min(opts && opts.max != null ? opts.max : 2, availW / Math.max(b.w, 1), availH / Math.max(b.h, 1))
    App.animateTo({ s, x: r.width / 2 - (b.x + b.w / 2) * s, y: top / 2 + r.height / 2 - (b.y + b.h / 2) * s }, opts && opts.instant ? 0 : 320)
  }
  App.contentBox = () => G.bounds(model.all().map((o) => R.box(o.id)).filter(Boolean))
  App.fitAll = (instant) => { const b = App.contentBox(); if (b) App.fitTo(b, { max: 1, instant }); else App.animateTo({ s: 1, x: rect().width / 2, y: rect().height / 2 }, instant ? 0 : 250) }

  // ---------- selection ----------
  // Selecting one member of a group selects the group.
  function expandGroups(ids) {
    const out = new Set(ids)
    for (const id of ids) { const o = model.get(id); if (o && o.group) for (const p of model.all()) if (p.group === o.group) out.add(p.id) }
    return out
  }
  App.select = (ids, opts) => {
    const next = opts && opts.raw ? new Set(ids) : expandGroups(ids)
    for (const id of App.sel) if (!next.has(id)) { const el = R.el(id); if (el) el.classList.remove('sel') }
    App.sel = next
    for (const id of next) { const el = R.el(id); if (el) el.classList.add('sel') }
    drawOverlay(); fire('selection')
  }
  App.selected = () => [...App.sel].map((id) => model.get(id)).filter(Boolean)
  App.selBox = () => G.bounds([...App.sel].map((id) => R.box(id)).filter(Boolean))

  // ---------- creating ----------
  // A new object id, minted once: <type letter>-<10 hex>, never one already on the board (an add with an existing id
  // counts as a replay and is skipped).
  App.mintId = (p) => { let id; do id = store.rid(p); while (model.get(id)); return id }
  App.newId = (type) => App.mintId(S.PREFIX[type] || 'o')
  App.create = (type, f, opts) => {
    const obj = Object.assign({ id: App.newId(type), type }, f)
    if (type === 'table' && !obj.rowIds) {   // a new table: its rows and columns get their own ids
      const rows = obj.rows || S.DEFAULTS.table.rows
      obj.rows = JSON.parse(JSON.stringify(rows)); obj.rowIds = rows.map(() => S.itemId('rw'))
      obj.colIds = Array.from({ length: Math.max(0, ...rows.map((r) => r.length)) }, () => S.itemId('cl'))
    }
    if (!model.apply([{ op: 'add', obj }])) return null
    if (!(opts && opts.keepTool)) App.setTool('select')
    App.select([obj.id], { raw: true })
    return obj.id
  }
  App.viewCentre = () => { const r = rect(); return App.toWorld(r.left + r.width / 2, r.top + r.height / 2) }
  // A free spot near p for a w×h object: nudge right/down until it doesn't overlap.
  App.freeSpot = (p, w, h) => {
    let x = p[0] - w / 2; let y = p[1] - h / 2
    const boxes = model.all().filter((o) => o.type !== 'frame' && o.type !== 'connector').map((o) => R.box(o.id))
    for (let i = 0; i < 40; i++) {
      const b = { x, y, w, h }
      if (!boxes.some((q) => G.intersects(b, q))) break
      x += w + 24; if (i % 5 === 4) { x = p[0] - w / 2; y += h + 24 }
    }
    return [Math.round(x), Math.round(y)]
  }

  // ---------- overlay: selection box, handles, hover, guides ----------
  const RESIZABLE = (o) => o && !o.locked && !['connector', 'comment'].includes(o.type)
  // An object's drawn bounds, rotation included (tables and text are as tall as they're drawn).
  App.drawnBox = (o) => {
    const b = R.box(o.id) || { x: o.x, y: o.y, w: o.w, h: o.h }; const a = ((o.rot || 0) % 180) * Math.PI / 180
    if (!a) return b
    const w = Math.abs(b.w * Math.cos(a)) + Math.abs(b.h * Math.sin(a)); const h = Math.abs(b.w * Math.sin(a)) + Math.abs(b.h * Math.cos(a))
    return { x: b.x + b.w / 2 - w / 2, y: b.y + b.h / 2 - h / 2, w, h }
  }
  const QUICK = ['sticky', 'shape', 'card']   // types a connection dot's click can quick-add from
  const ROTATABLE = (o) => o && !o.locked && ['sticky', 'text', 'shape', 'image', 'card', 'pen'].includes(o.type)
  let marquee = null; let guides = []; let tempConnector = null
  function el(cls, parent, style) { const e = document.createElement('div'); e.className = cls; if (style) Object.assign(e.style, style); (parent || overlay).appendChild(e); return e }
  function drawOverlay() {
    overlay.textContent = ''
    const r = rect()
    const sp = (x, y) => [x * App.cam.s + App.cam.x, y * App.cam.s + App.cam.y]
    // Hover: a thin outline and, on a single shape-like object, connection dots.
    const h = App.hover && model.get(App.hover)
    if (h && !App.sel.has(h.id) && !App.editing && App.tool === 'select' && h.type !== 'comment') {
      const b = R.box(h.id)
      if (b && h.type !== 'connector' && h.type !== 'pen') {
        const [x, y] = sp(b.x, b.y)
        el('hoverbox', overlay, { left: x + 'px', top: y + 'px', width: b.w * App.cam.s + 'px', height: b.h * App.cam.s + 'px', transform: h.rot ? 'rotate(' + h.rot + 'deg)' : '' })
      }
    }
    if (h && App.tool === 'select' && !App.editing && !h.locked && !(App.sel.has(h.id) && App.sel.size > 1) && ['sticky', 'shape', 'card', 'text', 'mind', 'image'].includes(h.type)) dots(h, sp)
    if (App.tool === 'line' && h && h.type !== 'connector' && h.type !== 'pen') dots(h, sp)
    // Selection.
    const list = App.selected()
    if (list.length && !App.editing && !(list.length === 1 && list[0].type === 'comment')) {
      if (list.length === 1 && list[0].type === 'connector') {
        const c = list[0]; const route = R.el(c.id) && R.el(c.id)._route
        if (route) {
          for (const [k, p] of [['from', route.pts[0]], ['to', route.pts[route.pts.length - 1]]]) {
            const [x, y] = sp(p[0], p[1])
            const hd = el('handle end', overlay, { left: x + 'px', top: y + 'px' }); hd.dataset.end = k
            if (c[k] && c[k].id) hd.classList.add('attached')
          }
        }
      } else {
        const b = App.selBox(); if (!b) return
        const one = list.length === 1 ? list[0] : null
        const rot = one && one.rot ? one.rot : 0
        const [x, y] = sp(b.x, b.y)
        const box = el('selbox' + (list.some((o) => o.locked) ? ' locked' : ''), overlay, { left: x + 'px', top: y + 'px', width: b.w * App.cam.s + 'px', height: b.h * App.cam.s + 'px', transform: rot ? 'rotate(' + rot + 'deg)' : '' })
        if (list.length > 1) for (const o of list) {
          const ob = R.box(o.id); if (!ob || o.type === 'connector') continue
          const [ox, oy] = sp(ob.x, ob.y)
          el('subbox', overlay, { left: ox + 'px', top: oy + 'px', width: ob.w * App.cam.s + 'px', height: ob.h * App.cam.s + 'px', transform: o.rot ? 'rotate(' + o.rot + 'deg)' : '' })
        }
        const canResize = one ? RESIZABLE(one) : list.every((o) => !o.locked)
        if (canResize) {
          const corners = one && (one.type === 'text') ? ['w', 'e'] : (one && ['sticky', 'image'].includes(one.type)) ? ['nw', 'ne', 'sw', 'se'] : ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w']
          for (const k of corners) { const hd = el('handle rz ' + k, box); hd.dataset.h = k }
        }
        if (one && ROTATABLE(one)) { const hd = el('handle rot', box); hd.dataset.h = 'rot' }
        if (one && one.locked) { const lk = el('lockbadge', box); lk.appendChild(g.BoardIcons.icon('lock', 12)) }
      }
    }
    for (const gd of guides) {
      if (gd.x != null) { const [x] = sp(gd.x, 0); el('guide v', overlay, { left: x + 'px', top: '0px', height: r.height + 'px' }) }
      else { const [, y] = sp(0, gd.y); el('guide h', overlay, { top: y + 'px', left: '0px', width: r.width + 'px' }) }
    }
    if (marquee) {
      const x = Math.min(marquee.x1, marquee.x2); const y = Math.min(marquee.y1, marquee.y2)
      el('marquee', overlay, { left: x - r.left + 'px', top: y - r.top + 'px', width: Math.abs(marquee.x2 - marquee.x1) + 'px', height: Math.abs(marquee.y2 - marquee.y1) + 'px' })
    }
    fire('overlay')
  }
  function dots(o, sp) {
    const b = R.box(o.id); if (!b) return
    for (const side of ['top', 'right', 'bottom', 'left']) {
      const p = G.anchorPoint(b, side); const [x, y] = sp(p[0], p[1])
      const off = 14
      const d = { top: [0, -off], right: [off, 0], bottom: [0, off], left: [-off, 0] }[side]
      const dot = el('cdot', overlay, { left: x + d[0] + 'px', top: y + d[1] + 'px' }); dot.dataset.side = side; dot.dataset.id = o.id
      if (QUICK.includes(o.type) && App.tool === 'select') { dot.classList.add('plus'); dot.title = 'Click: add a connected copy · drag: draw a line' }
    }
  }
  App.drawOverlay = drawOverlay

  // ---------- model → screen ----------
  model.onChange((ids, metaChanged) => {
    if (ids.size) R.update(ids)
    for (const id of [...App.sel]) if (!model.get(id)) App.sel.delete(id)
    for (const id of App.sel) { const e = R.el(id); if (e) e.classList.add('sel') }
    if (metaChanged) fire('meta')
    if (App.editing && ids.has(App.editing.id)) restyleEdit()
    drawOverlay(); fire('change', ids)
    const total = Object.keys(model.objects).length; if (total !== lastTotal) { lastTotal = total; App.countStickies() }   // only when objects come or go
  })

  // ---------- tools ----------
  App.setTool = (t, opts) => {
    App.tool = t; App.toolOpts = opts || {}
    viewport.dataset.tool = t
    if (t !== 'select') App.select([])
    fire('tool')
  }

  // ---------- pointer ----------
  let drag = null; let spaceDown = false
  App.objAt = (target) => objAt(target)   // the object a pointer event is on (a frame's body counts as canvas)
  const objAt = (target) => {
    const e = target && target.closest ? target.closest('.obj') : null
    if (!e) return null
    // A frame's body is canvas: only its title (or border) selects it.
    if (e.dataset.type === 'frame' && !target.closest('.ftitle')) return null
    return e.dataset.id
  }
  function pointerDown(e) {
    if (App.presenting) return
    if (e.button === 1 || (e.button === 0 && (spaceDown || App.tool === 'hand'))) { startPan(e); return }
    if (e.button !== 0) return
    // We manage focus ourselves; stop the browser's mousedown from moving it.
    if (!(e.target.closest && e.target.closest('textarea'))) e.preventDefault()
    // …which also keeps an old text selection (in a closed panel) alive; it would keep Cmd+C from copying objects.
    try { const ts = document.getSelection(); if (ts && !ts.isCollapsed && !App.editing) ts.removeAllRanges() } catch (_) {}
    viewport.focus({ preventScroll: true })
    // A checklist tick or a link badge on an object: its own click, not a select or a drag.
    const tick = e.target.closest && e.target.closest('.chk .box')
    if (tick && App.tool === 'select' && !App.editing) {
      const id = objAt(tick); const o = id && model.get(id)
      if (o && o.locked) { App.toast('Locked: unlock it to tick items'); return }
      if (o && Array.isArray(o.checks)) { const i = +tick.parentElement.dataset.i; model.apply([{ op: 'set', id, f: { checks: o.checks.map((c, j) => (j === i ? Object.assign({}, c, { done: !c.done }) : c)) } }]) }
      return
    }
    const lb = e.target.closest && e.target.closest('.linkb')
    if (lb && App.tool === 'select') { const host = lb.closest('.obj'); const id = host && host.dataset.id; if (id && model.get(id)) fire('link-open', id, lb); return }   // not objAt: a frame's body is canvas there
    const handle = e.target.closest && e.target.closest('.handle, .cdot')
    const [wx, wy] = App.toWorld(e.clientX, e.clientY)
    if (App.editing && !e.target.closest('textarea')) App.endEdit(true)

    if (handle && handle.classList.contains('cdot')) return startConnector(e, { id: handle.dataset.id, anchor: handle.dataset.side })
    if (handle && handle.dataset.end) return startEndDrag(e, handle.dataset.end)
    if (handle && handle.dataset.h === 'rot') return startRotate(e)
    if (handle && handle.dataset.h) return startResize(e, handle.dataset.h)

    // Vote badge: +1 (shift: −1) while voting is on.
    const vb = e.target.closest && e.target.closest('.votes.live')
    if (vb) { fire('vote', vb.closest('.obj').dataset.id, e.shiftKey ? -1 : 1); e.preventDefault(); return }
    const rc = e.target.closest && e.target.closest('.react')
    if (rc) { App.toggleReaction(rc.closest('.obj').dataset.id, rc.dataset.emoji); e.preventDefault(); return }

    const t = App.tool
    if (t === 'pen' || t === 'highlighter') return startPen(e, t === 'highlighter')
    if (t === 'eraser') return startErase(e)
    if (t === 'line') { const id = objAt(e.target); return startConnector(e, id ? { id, anchor: 'auto' } : { x: Math.round(wx), y: Math.round(wy) }) }
    if (t === 'comment') { fire('comment-new', [wx, wy], objAt(e.target)); return }
    if (['sticky', 'text', 'shape', 'frame', 'card', 'table', 'mind'].includes(t)) return startPlace(e, t, wx, wy)

    const id = objAt(e.target)
    if (id) {
      const o = model.get(id)
      if (o.type === 'comment') { App.select([id]); fire('comment-open', id); return }
      if (e.shiftKey || e.metaKey) {
        const next = new Set(App.sel); const grp = expandGroups([id])
        if (next.has(id)) for (const x of grp) next.delete(x); else for (const x of grp) next.add(x)
        App.select(next, { raw: true }); return
      }
      if (!App.sel.has(id)) App.select([id])
      startMove(e, wx, wy, e.altKey)
    } else {
      if (!e.shiftKey) App.select([])
      startMarquee(e)
    }
  }
  viewport.addEventListener('pointerdown', pointerDown)
  overlay.addEventListener('pointerdown', pointerDown)

  function capture(e) { try { viewport.setPointerCapture(e.pointerId) } catch (_) {} }
  window.addEventListener('pointermove', (e) => {
    if (drag) { drag.move(e); return }
    if (App.presenting) return
    const target = document.elementFromPoint(e.clientX, e.clientY)
    if (target && target.closest && target.closest('.cdot')) return
    const id = target && viewport.contains(target) ? objAt(target) : null
    if (id !== App.hover) { App.hover = id; drawOverlay() }
  })
  window.addEventListener('pointerup', (e) => { if (drag) { const d = drag; drag = null; d.up(e); guides = []; marquee = null; drawOverlay() } })
  window.addEventListener('pointercancel', () => { if (drag) { drag = null; model.cancelPreview(); guides = []; marquee = null; drawOverlay() } })

  function startPan(e) {
    capture(e); viewport.classList.add('panning')
    const sx = e.clientX; const sy = e.clientY; const cx = App.cam.x; const cy = App.cam.y
    drag = { move(m) { App.cam.x = cx + m.clientX - sx; App.cam.y = cy + m.clientY - sy; applyCam() }, up() { viewport.classList.remove('panning') } }
  }

  function startMarquee(e) {
    capture(e)
    const base = new Set(App.sel)
    marquee = { x1: e.clientX, y1: e.clientY, x2: e.clientX, y2: e.clientY }
    drag = {
      move(m) {
        marquee.x2 = m.clientX; marquee.y2 = m.clientY
        const [ax, ay] = App.toWorld(Math.min(marquee.x1, marquee.x2), Math.min(marquee.y1, marquee.y2))
        const [bx, by] = App.toWorld(Math.max(marquee.x1, marquee.x2), Math.max(marquee.y1, marquee.y2))
        const area = { x: ax, y: ay, w: bx - ax, h: by - ay }
        const hit = new Set(base)
        if (area.w * App.cam.s > 3 || area.h * App.cam.s > 3) for (const o of model.all()) {
          const b = R.box(o.id); if (!b) continue
          if (o.type === 'frame' ? G.inside(b, area) : G.intersects(b, area)) hit.add(o.id)
        }
        App.select(hit)
      },
      up() {},
    }
  }

  // Objects that travel with a move: the selection, plus everything inside a selected frame.
  function movers() {
    const ids = new Set([...App.sel].filter((id) => { const o = model.get(id); return o && !o.locked }))
    for (const id of [...ids]) {
      const f = model.get(id); if (!f || f.type !== 'frame') continue
      for (const o of model.all()) if (o.id !== id && !o.locked && o.type !== 'frame' && o.type !== 'connector' && G.inside(R.box(o.id), f)) ids.add(o.id)
    }
    return [...ids].map((id) => model.get(id)).filter((o) => o.type !== 'connector')
  }

  function startMove(e, wx, wy, dup) {
    capture(e)
    if (dup) { App.duplicate(0); }
    let list = movers()
    const start = new Map(list.map((o) => [o.id, { x: o.x, y: o.y }]))
    const box0 = App.selBox()
    const others = model.all().filter((o) => !start.has(o.id) && o.type !== 'connector' && o.type !== 'comment' && o.type !== 'pen').map((o) => R.box(o.id))
    let moved = false
    drag = {
      move(m) {
        const [mx, my] = App.toWorld(m.clientX, m.clientY)
        let dx = mx - wx; let dy = my - wy
        if (!moved && Math.hypot(dx * App.cam.s, dy * App.cam.s) < 3) return
        moved = true
        if (m.shiftKey) { if (Math.abs(dx) > Math.abs(dy)) dy = 0; else dx = 0 }
        if (box0) { const sn = snapMove(box0, dx, dy, others); dx = sn[0]; dy = sn[1] }
        for (const o of list) { const s = start.get(o.id); model.preview(o.id, { x: Math.round(s.x + dx), y: Math.round(s.y + dy) }) }
      },
      up() { if (moved) model.commitPreview(); else model.cancelPreview() },
    }
  }

  // Smart guides: snap the moving box's edges/centre to other objects (6 px on screen), or to the grid.
  function snapMove(b, dx, dy, others) {
    guides = []
    if (App.snap) { const gs = 24; return [Math.round((b.x + dx) / gs) * gs - b.x, Math.round((b.y + dy) / gs) * gs - b.y] }
    const tol = 6 / App.cam.s
    const xs = [b.x + dx, b.x + dx + b.w / 2, b.x + dx + b.w]; const ys = [b.y + dy, b.y + dy + b.h / 2, b.y + dy + b.h]
    let bestX = null; let bestY = null
    const view = App.viewBox()
    for (const o of others) {
      if (!o || !G.intersects(o, { x: view.x - view.w, y: view.y - view.h, w: view.w * 3, h: view.h * 3 })) continue
      for (const tx of [o.x, o.x + o.w / 2, o.x + o.w]) for (const sx of xs) { const d = tx - sx; if (Math.abs(d) < tol && (!bestX || Math.abs(d) < Math.abs(bestX.d))) bestX = { d, x: tx } }
      for (const ty of [o.y, o.y + o.h / 2, o.y + o.h]) for (const sy of ys) { const d = ty - sy; if (Math.abs(d) < tol && (!bestY || Math.abs(d) < Math.abs(bestY.d))) bestY = { d, y: ty } }
    }
    if (bestX) { dx += bestX.d; guides.push({ x: bestX.x }) }
    if (bestY) { dy += bestY.d; guides.push({ y: bestY.y }) }
    return [dx, dy]
  }

  function startResize(e, hk) {
    capture(e)
    const list = App.selected().filter((o) => o.type !== 'connector')
    const one = list.length === 1 ? list[0] : null
    const b0 = one ? { x: one.x, y: one.y, w: one.w, h: R.box(one.id).h } : App.selBox()
    const start = new Map(list.map((o) => [o.id, JSON.parse(JSON.stringify(o))]))
    const keepAspect = one && ['sticky', 'image'].includes(one.type)
    const rot = one && one.rot ? one.rot * Math.PI / 180 : 0
    const [sx0, sy0] = App.toWorld(e.clientX, e.clientY)
    drag = {
      move(m) {
        const [mx, my] = App.toWorld(m.clientX, m.clientY)
        // Work in the object's own (unrotated) frame.
        let dx = mx - sx0; let dy = my - sy0
        if (rot) { const c = Math.cos(-rot); const s = Math.sin(-rot); const ndx = dx * c - dy * s; dy = dx * s + dy * c; dx = ndx }
        let x = b0.x; let y = b0.y; let w = b0.w; let h = b0.h
        if (hk.includes('e')) w = b0.w + dx
        if (hk.includes('w')) { w = b0.w - dx; x = b0.x + dx }
        if (hk.includes('s')) h = b0.h + dy
        if (hk.includes('n')) { h = b0.h - dy; y = b0.y + dy }
        const minS = 16
        if ((keepAspect || m.shiftKey) && hk.length === 2) {
          const k = Math.max(w / b0.w, h / b0.h); w = b0.w * k; h = b0.h * k
          if (hk.includes('w')) x = b0.x + b0.w - w; if (hk.includes('n')) y = b0.y + b0.h - h
        }
        if (w < minS) { if (hk.includes('w')) x -= minS - w; w = minS }
        if (h < minS) { if (hk.includes('n')) y -= minS - h; h = minS }
        if (App.snap && !rot) { const gs = 24; const r2 = (v) => Math.round(v / gs) * gs; if (hk.includes('e')) w = r2(x + w) - x; if (hk.includes('s')) h = r2(y + h) - y }
        if (one) {
          if (rot) {
            // Keep the opposite corner still in world space.
            const c0 = [b0.x + b0.w / 2, b0.y + b0.h / 2]; const c1 = [x + w / 2, y + h / 2]
            const ddx = c1[0] - c0[0]; const ddy = c1[1] - c0[1]
            const rx = ddx * Math.cos(rot) - ddy * Math.sin(rot); const ry = ddx * Math.sin(rot) + ddy * Math.cos(rot)
            x = c0[0] + rx - w / 2; y = c0[1] + ry - h / 2
          }
          const f = { x: Math.round(x), y: Math.round(y), w: Math.round(w) }
          if (one.type !== 'text') f.h = Math.round(h)
          if (one.type === 'pen') { const s0 = start.get(one.id); f.points = s0.points.map((p) => [Math.round(p[0] * w / s0.w), Math.round(p[1] * h / s0.h)]) }
          model.preview(one.id, f)
        } else {
          const kx = w / b0.w; const ky = h / b0.h
          for (const o of list) {
            const s0 = start.get(o.id); const bx = R.box(o.id)
            const f = { x: Math.round(x + (s0.x - b0.x) * kx), y: Math.round(y + (s0.y - b0.y) * ky), w: Math.max(8, Math.round(s0.w * kx)) }
            if (o.type !== 'text') f.h = Math.max(8, Math.round((s0.h || bx.h) * ky))
            if (o.type === 'pen') f.points = s0.points.map((p) => [Math.round(p[0] * kx), Math.round(p[1] * ky)])
            if (o.type === 'text') f.fontSize = Math.max(6, Math.round((s0.fontSize || 24) * Math.min(kx, ky)))
            if (o.type !== 'comment') model.preview(o.id, f)
          }
        }
      },
      up() { model.commitPreview() },
    }
  }

  function startRotate(e) {
    capture(e)
    const o = App.selected()[0]; const b = R.box(o.id)
    const c = [b.x + b.w / 2, b.y + b.h / 2]
    drag = {
      move(m) {
        const [mx, my] = App.toWorld(m.clientX, m.clientY)
        let a = Math.atan2(my - c[1], mx - c[0]) * 180 / Math.PI + 90
        if (m.shiftKey) a = Math.round(a / 15) * 15
        else { const near = Math.round(a / 90) * 90; if (Math.abs(a - near) < 4) a = near }
        a = ((Math.round(a) % 360) + 360) % 360
        model.preview(o.id, { rot: a > 180 ? a - 360 : a })
        App.fire('rotating', a)
      },
      up() { model.commitPreview(); App.fire('rotating', null) },
    }
  }

  // Connector from an object (or a point) to wherever the pointer lets go.
  // The object under the pointer, looking past the line being dragged (its own stroke is on top).
  const objUnder = (m, skip) => { const live = skip && R.el(skip); for (const t of document.elementsFromPoint(m.clientX, m.clientY)) {
      if (live && live.contains(t)) continue
      if (overlay.contains(t)) { const d = t.closest('.cdot'); if (d && d.dataset.id && model.get(d.dataset.id)) return d.dataset.id; continue }   // a connection dot is its object; handles aren't
      return objAt(t)
    }
    return null
  }
  function startConnector(e, from) {
    capture(e)
    const last = App.lastConnector || {}
    const id = App.newId('connector')
    const obj = { id, type: 'connector', from, to: { x: 0, y: 0 }, route: last.route || 'elbow', endCap: last.endCap || 'arrow', stroke: last.stroke || 'ink', strokeWidth: last.strokeWidth || 2 }
    let started = false; const x0 = e.clientX; const y0 = e.clientY
    drag = {
      move(m) {
        if (!started && Math.hypot(m.clientX - x0, m.clientY - y0) < 5) return   // a click's wobble isn't a drag (it quick-adds)
        const [mx, my] = App.toWorld(m.clientX, m.clientY)
        const over = objUnder(m, id)
        obj.to = over && over !== from.id && over !== id ? { id: over, anchor: 'auto' } : { x: Math.round(mx), y: Math.round(my) }   // never the line's own loose end
        if (!started) { started = true; model.applyRemote({ ops: [{ op: 'add', obj: Object.assign({}, obj) }], by: store.me }) }
        else { const o = model.get(id); if (o) { o.to = obj.to; R.update(new Set([id])) } }
        App.hover = over; drawOverlay()
      },
      up() {
        const o = model.get(id)
        if (!o) { if (!started && from.id && from.anchor && App.tool === 'select') App.quickAdd(from.id, from.anchor); return }   // a click, not a drag
        // Replace the live preview with one real, undoable add.
        const fin = model.strip(o); model.applyRemote({ ops: [{ op: 'del', id }], by: store.me })
        // Dropped back on (or right by) the object it started from: no line.
        const src = fin.from.id && model.get(fin.from.id); const sb = src && App.drawnBox(src); const m = 26 / App.cam.s   // a dot reaches 23px out (14px offset + 9px radius)
        if (!fin.to.id && sb && fin.to.x > sb.x - m && fin.to.x < sb.x + sb.w + m && fin.to.y > sb.y - m && fin.to.y < sb.y + sb.h + m) return
        model.apply([{ op: 'add', obj: fin }])
        App.setTool('select'); App.select([id], { raw: true })
        if (!fin.to.id) fire('connector-dangling', id)
      },
    }
  }

  function startEndDrag(e, which) {
    capture(e)
    const c = App.selected()[0]
    drag = {
      move(m) {
        const [mx, my] = App.toWorld(m.clientX, m.clientY)
        const over = objUnder(m, c.id)
        const other = which === 'from' ? c.to : c.from
        const end = over && over !== c.id && !(other && other.id === over) ? { id: over, anchor: 'auto' } : { x: Math.round(mx), y: Math.round(my) }
        model.preview(c.id, { [which]: end })
        App.hover = over
      },
      up() { model.commitPreview() },
    }
  }

  function startPen(e, highlighter) {
    capture(e)
    const opts = App.toolOpts
    const pts = [App.toWorld(e.clientX, e.clientY).map(Math.round)]
    const NS = 'http://www.w3.org/2000/svg'
    const svg = document.createElementNS(NS, 'svg'); svg.setAttribute('class', 'pen-live' + (highlighter ? ' hl' : ''))
    const path = document.createElementNS(NS, 'path'); svg.appendChild(path); world.appendChild(svg)
    const color = R.colors.ink(opts.stroke || (highlighter ? 'yellow' : 'ink'))
    const width = highlighter ? (opts.width || 18) : (opts.width || 3)
    path.setAttribute('stroke', color); path.setAttribute('stroke-width', width)
    drag = {
      move(m) {
        const p = App.toWorld(m.clientX, m.clientY).map(Math.round); const q = pts[pts.length - 1]
        if (Math.hypot(p[0] - q[0], p[1] - q[1]) * App.cam.s < 1.5) return
        pts.push(p); path.setAttribute('d', G.penPath(pts))
      },
      up() {
        svg.remove()
        const s = G.simplify(pts, 0.6 / App.cam.s)
        const b = G.bounds(s.map((p) => ({ x: p[0], y: p[1], w: 0, h: 0 })))
        model.apply([{ op: 'add', obj: { id: App.newId('pen'), type: 'pen', x: b.x, y: b.y, w: Math.max(1, b.w), h: Math.max(1, b.h), points: s.map((p) => [p[0] - b.x, p[1] - b.y]), stroke: opts.stroke || (highlighter ? 'yellow' : 'ink'), strokeWidth: width, highlighter } }])
      },
    }
  }

  function startErase(e) {
    capture(e)
    const gone = new Set()
    const hitAt = (m) => {
      const [x, y] = App.toWorld(m.clientX, m.clientY); const r = 10 / App.cam.s
      for (const o of model.all()) {
        if (o.type !== 'pen' || gone.has(o.id) || !o.points || !o.points.length) continue
        if (x < o.x - r || y < o.y - r || x > o.x + o.w + r || y > o.y + o.h + r) continue
        if (G.distToPolyline([x - o.x, y - o.y], o.points.length > 1 ? o.points : [o.points[0], o.points[0]]) < r + (o.strokeWidth || 3) / 2) { gone.add(o.id); const el = R.el(o.id); if (el) el.style.opacity = '0.15' }
      }
    }
    hitAt(e)
    drag = { move: hitAt, up() { if (gone.size) model.apply([...gone].map((id) => ({ op: 'del', id }))) } }
  }

  // Click to drop a default-sized object, or drag to draw its box.
  function startPlace(e, t, wx, wy) {
    capture(e)
    const NS = 'http://www.w3.org/2000/svg'
    const ghost = document.createElement('div'); ghost.className = 'ghost t-' + t; world.appendChild(ghost)
    let x2 = wx; let y2 = wy; let dragged = false
    const drawable = ['shape', 'frame', 'text', 'table', 'card'].includes(t)
    drag = {
      move(m) {
        if (!drawable) return
        ;[x2, y2] = App.toWorld(m.clientX, m.clientY)
        if (Math.hypot((x2 - wx) * App.cam.s, (y2 - wy) * App.cam.s) > 6) dragged = true
        if (m.shiftKey) { const s = Math.max(Math.abs(x2 - wx), Math.abs(y2 - wy)); x2 = wx + Math.sign(x2 - wx || 1) * s; y2 = wy + Math.sign(y2 - wy || 1) * s }
        Object.assign(ghost.style, { left: Math.min(wx, x2) + 'px', top: Math.min(wy, y2) + 'px', width: Math.abs(x2 - wx) + 'px', height: Math.abs(y2 - wy) + 'px' })
      },
      up() {
        ghost.remove()
        const d = S.DEFAULTS[t]; const f = Object.assign({}, App.toolOpts.fields || {})
        let x; let y; let w = f.w || d.w; let h = f.h || d.h
        if (dragged) { x = Math.min(wx, x2); y = Math.min(wy, y2); w = Math.max(20, Math.abs(x2 - wx)); h = Math.max(20, Math.abs(y2 - wy)) }
        else { x = wx - w / 2; y = wy - h / 2 }
        if (App.snap) { x = Math.round(x / 24) * 24; y = Math.round(y / 24) * 24 }
        Object.assign(f, { x: Math.round(x), y: Math.round(y), w: Math.round(w), h: Math.round(h) })
        if (t === 'frame') { f.title = 'Frame ' + (model.all().filter((o) => o.type === 'frame').length + 1); f.order = model.all().filter((o) => o.type === 'frame').length; f.z = 0 }
        if (t === 'text' && !f.text) f.text = ''
        if (t === 'mind') f.text = 'Idea'
        const keep = e.shiftKey || App.toolOpts.sticky
        const id = App.create(t, f, { keepTool: keep })
        if (['sticky', 'text', 'shape', 'mind'].includes(t)) App.startEdit(id)
        else if (t === 'card') App.startEdit(id, 'title')
        else if (t === 'frame') App.startEdit(id, 'title')
      },
    }
  }

  // Wheel: scroll pans, pinch / ctrl / cmd zooms.
  viewport.addEventListener('wheel', (e) => {
    e.preventDefault()
    if (App.presenting) return
    if (e.ctrlKey || e.metaKey) App.zoomAt(e.clientX, e.clientY, App.cam.s * Math.exp(-e.deltaY * (e.deltaMode ? 0.05 : 0.0085)))
    else { const k = e.deltaMode ? 30 : 1; App.cam.x -= (e.shiftKey && !e.deltaX ? e.deltaY : e.deltaX) * k; App.cam.y -= (e.shiftKey && !e.deltaX ? 0 : e.deltaY) * k; applyCam() }
  }, { passive: false })

  viewport.addEventListener('dblclick', (e) => {
    if (App.presenting) return
    // The first click captured the pointer, so ask what is really under it.
    const under = document.elementFromPoint(e.clientX, e.clientY) || e.target
    if (under.closest && under.closest('textarea')) return
    const id = objAt(under)
    const [wx, wy] = App.toWorld(e.clientX, e.clientY)
    if (id) {
      const o = model.get(id)
      if (o.locked) { App.toast('Locked: unlock it to edit'); return }
      if (o.type === 'table') { const cell = under.closest('.cell'); if (cell) App.startEdit(id, 'cell', { r: +cell.dataset.r, c: +cell.dataset.c }); return }
      if (o.type === 'card') { App.startEdit(id, under.closest('.cdesc') ? 'text' : 'title'); return }
      if (o.type === 'frame') { App.startEdit(id, 'title'); return }
      if (o.type === 'connector') { App.startEdit(id, 'label'); return }
      if (['sticky', 'text', 'shape', 'mind'].includes(o.type)) App.startEdit(id)
      return
    }
    if (App.tool === 'select') {
      const [x, y] = [Math.round(wx - 100), Math.round(wy - 100)]
      const nid = App.create('sticky', { x, y, color: App.lastSticky || 'yellow' })
      App.startEdit(nid)
    }
  })

  // ---------- text editing ----------
  App.startEdit = (id, field, cell) => {
    const o = model.get(id); const o0 = o; if (!o || o.locked) return
    App.endEdit(true)
    const host = R.el(id); if (!host) return
    field = field || (o.type === 'frame' ? 'title' : o.type === 'connector' ? 'label' : 'text')
    let target; let value
    if (field === 'cell') { target = host.querySelector('.cell[data-r="' + cell.r + '"][data-c="' + cell.c + '"]'); value = (o.rows[cell.r] || [])[cell.c] || '' }
    else if (o.type === 'card') { target = host.querySelector(field === 'title' ? '.ctitle' : '.cdesc'); value = o[field] || '' }
    else if (field === 'title') { target = host.querySelector('.ftitle'); value = o.title || '' }
    else if (field === 'label') { target = host.querySelector('.label'); if (!target) { const l = document.createElement('div'); l.className = 'label editing'; const m = G.midpoint(host._route.pts); l.style.left = (m[0] - parseFloat(host.style.left)) + 'px'; l.style.top = (m[1] - parseFloat(host.style.top)) + 'px'; host.appendChild(l); target = l } value = o.label || '' }
    else { target = host.querySelector('.txt'); value = o.text || '' }
    if (!target) return
    const ta = document.createElement('textarea')
    ta.className = 'editor ed-' + o.type + ' ed-' + field
    ta.value = value; ta.spellcheck = true
    const cs = getComputedStyle(target)
    for (const k of ['fontSize', 'fontWeight', 'fontStyle', 'fontFamily', 'textAlign', 'color', 'letterSpacing', 'padding']) ta.style[k] = cs[k]
    ta.style.lineHeight = lineRatio(cs)
    target.classList.add('editing-hidden')
    target.parentNode.insertBefore(ta, target.nextSibling)
    Object.assign(ta.style, { left: target.offsetLeft + 'px', top: target.offsetTop + 'px', width: target.offsetWidth + 'px', height: Math.max(target.offsetHeight, parseFloat(cs.lineHeight) || 20) + 'px' })
    // Stickies and shapes centre their text: the editor grows with what is typed, around the same middle.
    const mid = target.offsetTop + target.offsetHeight / 2
    const room = o.type === 'sticky' ? [14, target.offsetParent ? target.offsetParent.clientHeight - 14 : target.offsetHeight] : [target.offsetTop, target.offsetTop + target.offsetHeight]
    App.editing = { id, field, cell, ta, target, orig: value }
    host.classList.add('is-editing')
    const grow = App.editing.grow = () => {
      const o = model.get(id) || o0
      if (o.type === 'text' || field === 'title' || field === 'label' || o.type === 'mind' || field === 'cell' || o.type === 'card') { ta.style.height = 'auto'; ta.style.height = ta.scrollHeight + 'px' }
      if (o.type === 'sticky' && !o.fontSize) { const live = Object.assign({}, o, { text: ta.value }); ta.style.fontSize = fitSize(live) + 'px' }
      if ((o.type === 'sticky' || o.type === 'shape') && field === 'text') {
        ta.style.height = 'auto'
        const h = Math.min(ta.scrollHeight, room[1] - room[0])
        ta.style.height = h + 'px'; ta.style.top = Math.max(room[0], Math.min(room[1] - h, mid - h / 2)) + 'px'
      }
      if (o.type === 'text') { const w = Math.max(o.w, Math.min(1200, measureWidth(ta))); if (w !== o.w) { host.style.width = w + 'px'; ta.style.width = w + 'px' } }
    }
    ta.addEventListener('input', grow); grow()
    ta.addEventListener('keydown', (e) => {
      e.stopPropagation()
      if (sizeKey(e)) return
      if (e.key === 'Escape') { e.preventDefault(); App.endEdit(true); App.select([id], { raw: true }) }
      else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); App.endEdit(true); App.select([id], { raw: true }) }
      else if (o.type === 'mind' && e.key === 'Tab') { e.preventDefault(); App.endEdit(true); fire('mind-child', id) }
      else if (o.type === 'mind' && e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); App.endEdit(true); fire('mind-sibling', id) }
      else if (field === 'cell' && e.key === 'Tab') {
        e.preventDefault(); App.endEdit(true)
        const cols = Math.max(...o.rows.map((r) => r.length)); let r = cell.r; let c = cell.c + (e.shiftKey ? -1 : 1)
        if (c >= cols) { c = 0; r++ } if (c < 0) { c = cols - 1; r-- }
        if (r >= 0 && r < o.rows.length) App.startEdit(id, 'cell', { r, c })
      } else if ((field === 'title' || field === 'label') && e.key === 'Enter') { e.preventDefault(); App.endEdit(true); App.select([id], { raw: true }) }
    })
    ta.addEventListener('blur', () => setTimeout(() => { if (App.editing && App.editing.ta === ta) App.endEdit(true) }, 0))
    ta.addEventListener('pointerdown', (e) => e.stopPropagation())
    ta.addEventListener('wheel', (e) => e.stopPropagation(), { passive: true })
    ta.focus({ preventScroll: true }); ta.select()
    drawOverlay(); fire('selection')
  }
  // A style change to the object being edited (size, bold, align from the bar) reaches the editor.
  function restyleEdit() {
    const ed = App.editing; const cs = getComputedStyle(ed.target)
    for (const k of ['fontSize', 'fontWeight', 'fontStyle', 'textAlign', 'color']) ed.ta.style[k] = cs[k]
    ed.ta.style.lineHeight = lineRatio(cs)
    ed.grow()
  }
  // Line height as a ratio, so it follows the font size when auto-fit changes it while typing.
  function lineRatio(cs) { const lh = parseFloat(cs.lineHeight); const fs = parseFloat(cs.fontSize); return lh && fs ? String(Math.round((lh / fs) * 1000) / 1000) : '1.3' }

  // ---------- text size ----------
  // One ladder for every type with a font size, so mixed selections step together.
  const SIZES = [10, 12, 14, 16, 18, 20, 24, 28, 32, 36, 40, 48, 56, 64, 72, 96, 120]
  App.SIZED = ['sticky', 'text', 'shape']
  App.textTargets = () => (App.editing ? [model.get(App.editing.id)] : App.selected()).filter((o) => o && !o.locked && App.SIZED.includes(o.type))
  // The size a reader sees: the set size, or for an auto-fit sticky the fitted one.
  App.shownSize = (o) => {
    if (o.fontSize) return o.fontSize
    const ed = App.editing && App.editing.id === o.id ? App.editing.ta : null
    const t = ed || (R.el(o.id) && R.el(o.id).querySelector('.txt'))
    return (t && parseFloat(getComputedStyle(t).fontSize)) || 18
  }
  App.textSize = (dir) => {
    const list = App.textTargets(); if (!list.length) return false
    model.apply(list.map((o) => {
      const cur = App.shownSize(o)
      const next = dir > 0 ? (SIZES.find((v) => v > cur) || SIZES[SIZES.length - 1]) : (SIZES.slice().reverse().find((v) => v < cur) || SIZES[0])
      return { op: 'set', id: o.id, f: { fontSize: next } }
    }))
    return true
  }
  // ⇧⌘. bigger, ⇧⌘, smaller (by key position, so any layout works), while typing or with a selection.
  function sizeKey(e) {
    if (!(e.metaKey || e.ctrlKey) || !e.shiftKey || (e.code !== 'Period' && e.code !== 'Comma')) return false
    e.preventDefault(); App.textSize(e.code === 'Period' ? 1 : -1); return true
  }

  function measureWidth(ta) {
    const m = document.querySelector('.measure-line') || Object.assign(document.createElement('div'), { className: 'measure-line' })
    if (!m.isConnected) document.body.appendChild(m)
    m.style.font = getComputedStyle(ta).font
    let w = 0; for (const line of ta.value.split('\n')) { m.textContent = line || ' '; w = Math.max(w, m.offsetWidth) }
    return w + 8
  }
  function fitSize(o) {
    const m = document.querySelector('.measure')
    const availW = o.w - 28; const availH = o.h - 28
    m.style.width = availW + 'px'; m.textContent = o.text || 'M'
    let lo = 8; let hi = Math.min(56, Math.round(Math.min(o.h, o.w * 1.2) / 6.2)); let best = lo
    while (lo <= hi) { const mid = (lo + hi) >> 1; m.style.fontSize = mid + 'px'; if (m.scrollHeight <= availH && m.scrollWidth <= availW + 1) { best = mid; lo = mid + 1 } else hi = mid - 1 }
    return best
  }
  App.endEdit = (save) => {
    const ed = App.editing; if (!ed) return
    App.editing = null
    const o = model.get(ed.id); const v = ed.ta.value
    ed.ta.remove(); ed.target.classList.remove('editing-hidden')
    const host = R.el(ed.id); if (host) host.classList.remove('is-editing')
    if (ed.target.classList.contains('label') && ed.target.classList.contains('editing')) ed.target.remove()
    if (!o) return
    if (save && v !== ed.orig) {
      let f
      if (ed.field === 'cell') { const rows = o.rows.map((r) => r.slice()); rows[ed.cell.r][ed.cell.c] = v; f = { rows } }
      else f = { [ed.field]: v }
      if (o.type === 'text') { const el = R.el(o.id); f.w = Math.round(parseFloat(el.style.width) || o.w); }
      model.apply([{ op: 'set', id: o.id, f }])
      if (o.type === 'text') { const el = R.el(o.id); if (el) model.apply([{ op: 'set', id: o.id, f: { h: el.offsetHeight } }], { noUndo: true }) }
    } else if (o.type === 'text' && !v.trim() && !ed.orig) {
      model.apply([{ op: 'del', id: o.id }], { noUndo: true })
    } else R.update(new Set([o.id]))
    drawOverlay(); fire('selection')
  }
  store.setHold(() => !!App.editing)

  // ---------- actions ----------
  App.del = () => {
    const ids = [...App.sel].filter((id) => !(model.get(id) || {}).locked)
    if (!ids.length) { if (App.sel.size) App.toast('Locked items can’t be deleted'); return }
    // Connectors attached to deleted objects go too; so do mind-map children.
    const all = new Set(ids)
    const addKids = (id) => { for (const k of R.childrenOf(id)) if (!all.has(k)) { all.add(k); addKids(k) } }
    ids.forEach(addKids)
    for (const id of [...all]) for (const c of R.connectorsOf(id)) all.add(c)
    for (const o of model.all()) if (o.type === 'comment' && all.has(o.target)) all.add(o.id)   // the agent CLI does the same
    model.apply([...all].map((id) => ({ op: 'del', id })))
    App.select([])
  }
  // Copy objects with fresh ids, keeping connectors, groups and mind links inside the set.
  function cloneSet(objs, dx, dy) {
    const map = new Map(); const groups = new Map()
    for (const o of objs) map.set(o.id, App.newId(o.type))
    const ops = []; let z = model.maxZ()
    for (const o of objs) {
      const c = model.strip(o); c.id = map.get(o.id); c.z = ++z
      if (o.type !== 'connector') { c.x = Math.round(o.x + dx); c.y = Math.round(o.y + dy) }
      if (c.group) { if (!groups.has(c.group)) groups.set(c.group, store.rid('g')); c.group = groups.get(c.group) }
      if (o.type === 'connector') for (const k of ['from', 'to']) { const e = c[k]; if (e && e.id) { if (map.has(e.id)) c[k] = { id: map.get(e.id), anchor: e.anchor }; else { const t = model.get(e.id); c[k] = t ? { x: Math.round(t.x + t.w / 2 + dx), y: Math.round(t.y + t.h / 2 + dy) } : Number.isFinite(e.x) ? { x: e.x + dx, y: e.y + dy } : { id: e.id, anchor: e.anchor } } } else if (e) c[k] = { x: e.x + dx, y: e.y + dy } }
      if (o.type === 'mind' && o.parent) c.parent = map.get(o.parent) || ''
      if (o.type === 'comment' && o.target) c.target = map.get(o.target) || ''
      delete c.votes; delete c.reactions
      ops.push({ op: 'add', obj: c })
    }
    return { ops, ids: [...map.values()] }
  }
  // Quick add: a click on a connection dot (no drag) puts an empty copy of the object on that side, joined by a line,
  // and starts typing in it. The fast way to build a flowchart by hand. Past anything already there, it steps further out.
  App.quickAdd = (id, side) => {
    const o = model.get(id); if (!o || o.locked || !QUICK.includes(o.type)) return null
    const dir = { top: [0, -1], right: [1, 0], bottom: [0, 1], left: [-1, 0] }[side]; if (!dir) return null
    const GAP = 80; const opp = { top: 'bottom', bottom: 'top', left: 'right', right: 'left' }[side]
    const c = model.strip(o); c.id = App.newId(o.type); c.z = model.maxZ() + 1
    for (const k of ['votes', 'reactions', 'group', 'tags', 'rot']) delete c[k]
    for (const k of Object.keys(c)) if (S.isExtraKey(o.type, k) || ['checks', 'link', 'crop'].includes(k)) delete c[k]   // an empty copy: no checklist, link or crop from the original
    if ('text' in c) c.text = ''; if (o.type === 'card') { c.title = ''; c.text = ''; c.status = ''; c.assignee = ''; c.due = '' }
    const free = (x, y) => !model.all().some((q) => { if (q.id === o.id || ['connector', 'frame', 'pen', 'comment'].includes(q.type)) return false; const b = App.drawnBox(q); return x < b.x + b.w && x + o.w > b.x && y < b.y + b.h && y + o.h > b.y })
    let x = o.x; let y = o.y
    for (let k = 1; k <= 8; k++) { x = o.x + dir[0] * k * (o.w + GAP); y = o.y + dir[1] * k * (o.h + GAP); if (free(x, y)) break }
    c.x = Math.round(x); c.y = Math.round(y)
    const last = App.lastConnector || {}
    const line = { id: App.newId('connector'), type: 'connector', from: { id: o.id, anchor: side }, to: { id: c.id, anchor: opp }, route: last.route || 'elbow', endCap: last.endCap || 'arrow', stroke: last.stroke || 'ink', strokeWidth: last.strokeWidth || 2, z: c.z + 1 }
    if (!model.apply([{ op: 'add', obj: c }, { op: 'add', obj: line }])) return null   // one undo step
    App.select([c.id], { raw: true })
    const b = R.box(c.id); const r = rect(); const [sx, sy] = App.toScreen(b.x, b.y); const [ex, ey] = App.toScreen(b.x + b.w, b.y + b.h)
    if (sx < 60 || sy < 60 || ex > r.width - 60 || ey > r.height - 80) App.animateTo({ s: App.cam.s, x: r.width / 2 - (b.x + b.w / 2) * App.cam.s, y: r.height / 2 - (b.y + b.h / 2) * App.cam.s }, 200)
    App.startEdit(c.id, o.type === 'card' ? 'title' : undefined)
    return c.id
  }
  // Copy style / paste style: the look, not the content. A field carries over only where the other type has the
  // same kind of field (a sticky's paper colour never lands on a shape's text colour).
  const STYLE = ['color', 'fill', 'stroke', 'strokeWidth', 'dash', 'fontSize', 'align', 'bold', 'italic', 'route', 'startCap', 'endCap']
  let style = null
  App.hasStyle = () => !!style
  // From the one selected object, or (a group selected as a whole) the member under the pointer or clicked.
  App.styleSource = (id) => {
    const list = App.selected(); if (id && App.sel.has(id)) return model.get(id)
    if (list.length === 1) return list[0]
    if (App.hover && App.sel.has(App.hover)) return model.get(App.hover)
    return list.length && list.every((o) => o.group && o.group === list[0].group) ? list.find((o) => o.type !== 'connector') || null : null
  }
  App.copyStyle = (id) => {
    const o = App.styleSource(id); if (!o) { App.toast('Select one object to copy its style'); return }
    const f = {}; for (const k of STYLE) if (k in o && S.TYPES[o.type] && S.TYPES[o.type][k]) f[k] = o[k]
    style = { type: o.type, f }; App.toast('Style copied: select objects, then Paste style (⌥⌘V)')
  }
  App.pasteStyle = () => {
    if (!style) return
    const src = S.TYPES[style.type] || {}; let n = 0
    const ops = App.selected().filter((o) => !o.locked).map((o) => {
      const t = S.TYPES[o.type] || {}; const f = {}
      for (const [k, v] of Object.entries(style.f)) if (t[k] && JSON.stringify(t[k]) === JSON.stringify(src[k]) && o[k] !== v) f[k] = v
      if (Object.keys(f).length) n++
      return { op: 'set', id: o.id, f }
    }).filter((op) => Object.keys(op.f).length)
    if (ops.length) model.apply(ops); else App.toast('Nothing to change: those objects don’t share that style')
    return n
  }
  // Tidy up: a messy selection into neat rows, keeping the rows people made (by height) and their order in each,
  // evenly spaced from the top-left of the selection. Lines follow their ends; frames, drawings and pins stay put.
  const TIDY = (o) => !o.locked && !['connector', 'frame', 'pen', 'comment'].includes(o.type)
  App.canTidy = () => App.selected().filter(TIDY).length > 1
  App.tidy = () => {
    const list = App.selected().filter(TIDY).map((o) => ({ o, b: App.drawnBox(o) })); if (list.length < 2) return
    const GAP = 24
    const hs = list.map((x) => x.b.h).sort((a, b) => a - b); const mid = hs[hs.length >> 1]
    const cy = (x) => x.b.y + x.b.h / 2
    const byY = list.slice().sort((a, b) => cy(a) - cy(b))
    const rows = []; for (const it of byY) { const r = rows[rows.length - 1]; if (r && cy(it) - r.cy < mid * 0.6) r.items.push(it); else rows.push({ cy: cy(it), items: [it] }) }
    let y = Math.min(...list.map((x) => x.b.y)); const x0 = Math.min(...list.map((x) => x.b.x)); const ops = []
    for (const r of rows) {
      r.items.sort((a, b) => a.b.x - b.b.x); let x = x0; const rh = Math.max(...r.items.map((it) => it.b.h))
      for (const { o, b } of r.items) {
        // place the drawn box; the stored x/y keep their offset from it (rotation, measured height)
        const f = { x: Math.round(o.x + (x - b.x)), y: Math.round(o.y + (y + (rh - b.h) / 2 - b.y)) }
        if (f.x !== o.x || f.y !== o.y) ops.push({ op: 'set', id: o.id, f }); x += b.w + GAP
      }
      y += rh + GAP
    }
    if (ops.length) model.apply(ops)
  }
  App.withConnectors = (objs) => {
    const ids = new Set(objs.map((o) => o.id))
    const extra = model.all().filter((c) => c.type === 'connector' && !ids.has(c.id) && c.from && c.to && ids.has(c.from.id) && ids.has(c.to.id))
    return objs.concat(extra)
  }
  App.duplicate = (off) => {
    const objs = App.withConnectors(App.selected()); if (!objs.length) return
    const d = off == null ? 24 : off
    const { ops, ids } = cloneSet(objs, d, d)
    if (model.apply(ops)) { App.select(ids, { raw: true }); fire('copied', ids) }
  }
  let clip = null
  const CLIP = '⧉boardclip '
  App.copy = (cut) => {
    const objs = App.withConnectors(App.selected()); if (!objs.length) return
    clip = objs.map((o) => model.strip(o))
    try { navigator.clipboard && navigator.clipboard.writeText(CLIP + JSON.stringify(clip)).catch(() => {}) } catch (_) {}
    if (cut) App.del()
  }
  // The copy/cut events, not just the keys: in the Garden the host app's Edit menu takes Cmd+C/X
  // (the keydown never arrives, only the event), and the async clipboard can be blocked in the frame.
  // Writing the event's own clipboardData needs neither.
  let copies = 0; let forced = false
  const onCopy = (cut) => (e) => {
    copies++   // handled here, even when left to the browser: the key's fallback must not act (a cut would delete)
    if (App.editing || (!forced && e.target && /INPUT|TEXTAREA/.test(e.target.tagName))) return
    const sel = document.getSelection && document.getSelection()
    if (!forced && sel && !sel.isCollapsed && String(sel).trim()) return   // text selected in a panel: the browser copies it
    const objs = App.withConnectors(App.selected()); if (!objs.length || !e.clipboardData) return
    clip = objs.map((o) => model.strip(o))
    e.clipboardData.setData('text/plain', CLIP + JSON.stringify(clip)); e.preventDefault()
    if (cut) App.del()
  }
  // From a menu: the same event (so the system clipboard gets it), or the old way if the frame refuses it.
  App.copyCmd = (cut) => {
    const n = copies; forced = true
    try { document.execCommand(cut ? 'cut' : 'copy') } catch (_) {} finally { forced = false }
    if (copies === n) App.copy(cut)
  }
  // WebKit fires copy/cut only when something is selectable to copy, unless beforecopy says yes.
  const allow = (e) => {
    const sel = document.getSelection && document.getSelection()
    if (App.editing || (!forced && e.target && /INPUT|TEXTAREA/.test(e.target.tagName)) || (!forced && sel && !sel.isCollapsed && String(sel).trim())) return
    if (App.sel.size) e.preventDefault()
  }
  document.addEventListener('beforecopy', allow)
  document.addEventListener('beforecut', allow)
  document.addEventListener('copy', onCopy(false))
  document.addEventListener('cut', onCopy(true))
  App.hasClip = () => !!(clip && clip.length)   // something copied on this board page (the right-click menu's Paste)
  App.pasteClip = (at) => App.pasteObjs(clip, at)
  App.pasteObjs = (objs, at) => {
    if (!objs || !objs.length) return
    const b = G.bounds(objs.filter((o) => o.type !== 'connector').map((o) => ({ x: o.x, y: o.y, w: o.w || 0, h: o.h || 0 }))) || { x: 0, y: 0, w: 0, h: 0 }
    const p = at || App.viewCentre()
    const { ops, ids } = cloneSet(objs, Math.round(p[0] - b.x - b.w / 2), Math.round(p[1] - b.y - b.h / 2))
    if (model.apply(ops)) { App.select(ids, { raw: true }); fire('copied', ids) }
  }
  document.addEventListener('paste', (e) => {
    if (App.editing || (e.target && /INPUT|TEXTAREA/.test(e.target.tagName))) return
    const dt = e.clipboardData
    const file = dt && [...dt.items].find((i) => i.kind === 'file' && /^image\//.test(i.type))
    if (file) { e.preventDefault(); fire('paste-image', file.getAsFile()); return }
    const text = dt && dt.getData('text/plain')
    if (text && text.startsWith(CLIP)) {
      e.preventDefault()
      let objs = null; try { objs = JSON.parse(text.slice(CLIP.length)) } catch (_) { App.toast('That copied board content is damaged: nothing was pasted'); return }
      App.pasteObjs(objs); return
    }
    if (text && text.trim()) {
      e.preventDefault()
      const lines = text.split(/\n+/).map((s) => s.trim()).filter(Boolean)
      const c = App.viewCentre()
      if (lines.length > 1 && lines.length <= 40 && lines.every((l) => l.length < 200)) {
        const cols = Math.ceil(Math.sqrt(lines.length)); const ops = []; const ids = []
        lines.forEach((t, i) => { const id = App.newId('sticky'); ids.push(id); ops.push({ op: 'add', obj: { id, type: 'sticky', text: t, color: App.lastSticky || 'yellow', x: Math.round(c[0] - cols * 110 + (i % cols) * 220), y: Math.round(c[1] - 110 + Math.floor(i / cols) * 220) } }) })
        if (model.apply(ops)) { App.select(ids, { raw: true }); fire('copied', ids) }
      } else App.create('text', { x: Math.round(c[0] - 200), y: Math.round(c[1] - 20), w: 400, text, fontSize: 20 })
      return
    }
    if (clip) { e.preventDefault(); App.pasteObjs(clip) }
  })
  App.pasteInternal = () => { if (clip) App.pasteObjs(clip) }

  App.group = () => {
    const list = App.selected(); if (list.length < 2) return
    const gid = store.rid('g'); model.apply(list.map((o) => ({ op: 'set', id: o.id, f: { group: gid } })))
  }
  App.ungroup = () => { const list = App.selected().filter((o) => o.group); if (list.length) model.apply(list.map((o) => ({ op: 'set', id: o.id, f: { group: '' } }))) }
  App.toggleLock = () => {
    const list = App.selected(); if (!list.length) return
    const lock = !list.every((o) => o.locked)
    model.apply(list.map((o) => ({ op: 'set', id: o.id, f: { locked: lock } }))); App.toast(lock ? 'Locked' : 'Unlocked')
  }
  // Layers: forward/back one step past the nearest overlapping object, or all the way.
  App.layer = (dir) => {
    const list = App.selected().filter((o) => o.type !== 'frame'); if (!list.length) return
    const others = model.all().filter((o) => o.type !== 'frame' && !App.sel.has(o.id)).sort((a, b) => a.z - b.z)
    const ops = []
    if (dir === 'front' || dir === 'back') {
      let z = dir === 'front' ? model.maxZ() : model.minZ()
      const sorted = list.slice().sort((a, b) => (dir === 'front' ? a.z - b.z : b.z - a.z))
      for (const o of sorted) ops.push({ op: 'set', id: o.id, f: { z: dir === 'front' ? ++z : --z } })
    } else {
      const b = App.selBox(); const zs = list.map((o) => o.z)
      const over = others.filter((o) => G.intersects(R.box(o.id), b))
      const t = dir === 'forward' ? over.find((o) => o.z > Math.max(...zs)) : over.slice().reverse().find((o) => o.z < Math.min(...zs))
      if (!t) return
      for (const o of list) ops.push({ op: 'set', id: o.id, f: { z: dir === 'forward' ? t.z + 0.5 + o.z / 1e6 : t.z - 0.5 - (1e6 - o.z) / 1e9 } })
    }
    model.apply(ops)
  }
  App.align = (kind) => {
    const list = App.selected().filter((o) => o.type !== 'connector' && !o.locked); if (list.length < 2) return
    const b = G.bounds(list.map((o) => R.box(o.id)))
    model.apply(list.map((o) => {
      const ob = R.box(o.id); const f = {}
      if (kind === 'left') f.x = b.x; if (kind === 'right') f.x = Math.round(b.x + b.w - ob.w); if (kind === 'center') f.x = Math.round(b.x + b.w / 2 - ob.w / 2)
      if (kind === 'top') f.y = b.y; if (kind === 'bottom') f.y = Math.round(b.y + b.h - ob.h); if (kind === 'middle') f.y = Math.round(b.y + b.h / 2 - ob.h / 2)
      return { op: 'set', id: o.id, f }
    }))
  }
  App.distribute = (axis) => {
    const list = App.selected().filter((o) => o.type !== 'connector' && !o.locked); if (list.length < 3) return
    const k = axis === 'h' ? 'x' : 'y'; const d = axis === 'h' ? 'w' : 'h'
    const sorted = list.map((o) => ({ o, b: R.box(o.id) })).sort((a, b) => a.b[k] - b.b[k])
    const first = sorted[0].b; const last = sorted[sorted.length - 1].b
    const total = sorted.reduce((s, x) => s + x.b[d], 0)
    const gap = (last[k] + last[d] - first[k] - total) / (sorted.length - 1)
    let pos = first[k]; const ops = []
    for (const x of sorted) { ops.push({ op: 'set', id: x.o.id, f: { [k]: Math.round(pos) } }); pos += x.b[d] + gap }
    model.apply(ops)
  }
  App.nudge = (dx, dy) => {
    const list = movers(); if (!list.length) return
    model.apply(list.map((o) => ({ op: 'set', id: o.id, f: { x: o.x + dx, y: o.y + dy } })))
  }
  // Reactions are kept per person ({person: [emoji, …]}). The person's own are under person:owner; an older
  // entry under their plain name counts as theirs too, so a click takes it off wherever it is.
  App.toggleReaction = (id, emoji) => {
    const o = model.get(id); if (!o) return
    const r = JSON.parse(JSON.stringify(o.reactions || {}))
    const mine = Object.keys(r).filter((k) => model.isMe(k))
    if (mine.some((k) => (r[k] || []).includes(emoji))) {
      for (const k of mine) { r[k] = (r[k] || []).filter((x) => x !== emoji); if (!r[k].length) delete r[k] }
    } else r[model.me] = (r[model.me] || []).concat(emoji)
    model.apply([{ op: 'set', id, f: { reactions: r } }])
  }
  App.setFields = (f, filter) => {
    const list = (App.editing ? [model.get(App.editing.id)] : App.selected()).filter((o) => o && !o.locked && (!filter || filter(o))); if (!list.length) return
    model.apply(list.map((o) => {
      const spec = Object.assign({}, S.COMMON, S.TYPES[o.type]); const ff = {}
      for (const k of Object.keys(f)) if (spec[k]) ff[k] = f[k]
      return { op: 'set', id: o.id, f: ff }
    }).filter((op) => Object.keys(op.f).length))
  }

  // ---------- keyboard ----------
  // How the focused control got focus: a click leaves Space to pan and Enter to edit; the keyboard lets them press it.
  // (:focus-visible can't tell: Chrome turns it on as soon as a key is pressed.)
  // Whichever came last, a pointer press or a key press, says how focus (or a panel) got there.
  let seq = 0; let lastPointer = 0; let lastKey = 0; let pointerFocus = false; let tabbed = null   // tabbed: the object Tab last moved to
  document.addEventListener('pointerdown', (e) => { lastPointer = ++seq; if (viewport.contains(e.target) && e.button === 0 && !spaceDown && App.tool !== 'hand') tabbed = null }, true)   // a selecting click on the canvas (not a pan): Tab is the mind map's again
  document.addEventListener('keydown', (e) => { if (!/^(Shift|Meta|Control|Alt|CapsLock)$/.test(e.key)) lastKey = ++seq }, true)   // a modifier alone isn't keyboard use
  document.addEventListener('focusin', () => { pointerFocus = lastPointer > lastKey }, true)
  App.markPointerFocus = () => { pointerFocus = true }   // focus handed back to a control the mouse used (a panel's Escape)
  App.pointerJustNow = () => lastPointer > lastKey   // was this opened by a click (not a key)?
  let tabbing = false; App.on('selection', () => { if (!tabbing) tabbed = null })   // any other selection change: Tab is the mind map's again
  const TOOLKEYS = { v: 'select', h: 'hand', n: 'sticky', t: 'text', s: 'shape', r: 'shape:rect', o: 'shape:ellipse', l: 'line', p: 'pen', e: 'eraser', f: 'frame', c: 'comment', m: 'mind', k: 'card' }
  document.addEventListener('keydown', (e) => {
    const tag = e.target && e.target.tagName
    if (tag === 'TEXTAREA' || tag === 'INPUT') return
    // A control focused from the keyboard keeps its own keys: Space and Enter press it, arrows move in its menu.
    // (Focused by a click instead: Space still pans and Enter still edits.)
    const ctl = e.target !== viewport && e.target.closest && e.target.closest('button,select,a,[role=button],[role=menuitem],[role=menuitemradio],[role=treeitem],[role=option]')
    if (ctl && (e.key === ' ' || e.key === 'Enter' || (e.key.startsWith('Arrow') && ctl.closest('[role=menu],[role=tree],[role=toolbar],[role=listbox]'))) && !pointerFocus) return
    const mod = e.metaKey || e.ctrlKey; const k = e.key.toLowerCase()
    if (e.key === ' ' && !e.repeat) { spaceDown = true; viewport.classList.add('space'); e.preventDefault(); return }
    if (App.presenting) { fire('present-key', e); return }
    if (sizeKey(e)) return
    if (mod && k === 'z') { e.preventDefault(); if (e.shiftKey) model.redo(); else model.undo(); return }
    if (mod && k === 'y') { e.preventDefault(); model.redo(); return }
    if (mod && k === 'a') { e.preventDefault(); App.select(model.all().map((o) => o.id), { raw: true }); return }
    if (mod && e.altKey && e.code === 'KeyC') { e.preventDefault(); App.copyStyle(); return }   // e.code: Alt changes e.key on a Mac
    if (mod && e.altKey && e.code === 'KeyV') { e.preventDefault(); App.pasteStyle(); return }
    // The copy/cut event normally does it (it runs inside this key's default action); if none came, do it here.
    if (mod && (k === 'c' || k === 'x')) {
      // Text highlighted in a panel is the browser's: never a copy (or a cut!) of the selected objects.
      const ts = document.getSelection && document.getSelection(); if (ts && !ts.isCollapsed && String(ts).trim()) return
      const n = copies; setTimeout(() => { if (copies === n) App.copy(k === 'x') }, 0); return
    }
    if (mod && k === 'd') { e.preventDefault(); App.duplicate(); return }
    if (mod && k === 'g') { e.preventDefault(); if (e.shiftKey) App.ungroup(); else App.group(); return }
    if (mod && e.shiftKey && k === 'l') { e.preventDefault(); App.toggleLock(); return }
    if (mod && k === 'f') { e.preventDefault(); fire('search'); return }
    if (mod && e.key === ']') { e.preventDefault(); App.layer('front'); return }
    if (mod && e.key === '[') { e.preventDefault(); App.layer('back'); return }
    if (mod && (e.key === '=' || e.key === '+')) { e.preventDefault(); App.zoomCentre(1.25); return }
    if (mod && e.key === '-') { e.preventDefault(); App.zoomCentre(0.8); return }
    if (mod && e.key === '0') { e.preventDefault(); const r = rect(); App.animateTo({ s: 1, x: r.width / 2 - (r.width / 2 - App.cam.x) / App.cam.s, y: r.height / 2 - (r.height / 2 - App.cam.y) / App.cam.s }, 200); return }
    if (mod) return
    if (e.key === ']') { App.layer('forward'); return }
    if (e.key === '[') { App.layer('backward'); return }
    if (e.key === 'Delete' || e.key === 'Backspace') { e.preventDefault(); App.del(); return }
    if ((e.key === 'ContextMenu' || (e.shiftKey && e.key === 'F10')) && e.target === viewport) { e.preventDefault(); fire('canvas-menu'); return }
    if (e.key === 'Escape') { if (App.tool !== 'select') App.setTool('select'); else App.select([]); fire('escape'); return }
    if (e.key === 'Enter' && App.sel.size === 1) { e.preventDefault(); const o = App.selected()[0]; App.startEdit(o.id, o.type === 'card' || o.type === 'frame' ? 'title' : undefined); return }
    if (e.key === 'Tab' && !e.shiftKey && e.target === viewport && App.sel.size === 1 && App.selected()[0].type === 'mind' && !(tabbed && App.sel.has(tabbed))) { e.preventDefault(); fire('mind-child', App.selected()[0].id); return }
    // Tab on the canvas: the next object in reading order (rows, then left to right), panned into view.
    // Past the last one, Tab leaves the canvas as usual.
    if (e.key === 'Tab' && e.target === viewport) {
      // A group is one stop (selecting any member selects it all); a mind node reached this way keeps Tab for moving on.
      const seen = new Set(); const keyOf = (o) => o.group || o.id
      const list = model.all().map((o) => ({ o, b: R.box(o.id) })).filter((x) => x.b).sort((a, b) => Math.round(a.b.y / 80) - Math.round(b.b.y / 80) || a.b.x - b.b.x || a.b.y - b.b.y)
        .filter((x) => !seen.has(keyOf(x.o)) && seen.add(keyOf(x.o)))
      const selKeys = new Set(App.selected().map(keyOf)); const i = selKeys.size === 1 ? list.findIndex((x) => selKeys.has(keyOf(x.o))) : -1
      const j = i < 0 ? (e.shiftKey ? list.length - 1 : 0) : i + (e.shiftKey ? -1 : 1)
      if (!list[j]) { App.select([]); tabbed = null; return }
      e.preventDefault(); tabbing = true; try { App.select([list[j].o.id]) } finally { tabbing = false } tabbed = list[j].o.id
      const r = rect(); const b = list[j].b; const [sx, sy] = App.toScreen(b.x, b.y); const [ex, ey] = App.toScreen(b.x + b.w, b.y + b.h)
      if (sx < 60 || sy < 60 || ex > r.width - 60 || ey > r.height - 80) App.animateTo({ s: App.cam.s, x: r.width / 2 - (b.x + b.w / 2) * App.cam.s, y: r.height / 2 - (b.y + b.h / 2) * App.cam.s }, 200)
      return
    }
    if (e.key.startsWith('Arrow') && App.sel.size) {
      e.preventDefault(); const d = e.shiftKey ? 24 : 1
      App.nudge(e.key === 'ArrowLeft' ? -d : e.key === 'ArrowRight' ? d : 0, e.key === 'ArrowUp' ? -d : e.key === 'ArrowDown' ? d : 0); return
    }
    if (e.shiftKey && e.key === '!') { App.fitAll(); return }
    if (e.shiftKey && (e.key === '@' || e.code === 'Digit2')) { const b = App.selBox(); if (b) App.fitTo(b, { max: 2 }); return }
    if (e.shiftKey && e.code === 'Digit1') { App.fitAll(); return }
    if (e.key === '?') { fire('help'); return }
    if (e.shiftKey && k === 'p') { App.setTool('highlighter'); return }
    if (e.shiftKey && k === 'g') { App.snap = !App.snap; App.toast(App.snap ? 'Snap to grid on' : 'Snap to grid off'); fire('tool'); return }
    const t = TOOLKEYS[k]
    if (t && !e.shiftKey && !e.altKey) {
      if (t.startsWith('shape:')) App.setTool('shape', { fields: { shape: t.slice(6) } })
      else App.setTool(t, t === 'shape' ? { fields: { shape: App.lastShape || 'round' } } : undefined)
    }
  })
  document.addEventListener('keyup', (e) => { if (e.key === ' ') { spaceDown = false; viewport.classList.remove('space') } })
  window.addEventListener('blur', () => { spaceDown = false; viewport.classList.remove('space') })

  // ---------- small things ----------
  let toastTimer = 0
  // action: {label, run} adds a button (e.g. Undo); the toast then stays longer and takes clicks.
  let toastHide = null
  App.toast = (msg, action) => {
    const t = $('toast'); t.textContent = msg; t.hidden = false; t.classList.add('show'); t.classList.toggle('act', !!action)
    if (action) {
      const b = document.createElement('button'); b.type = 'button'; b.className = 'tact'; b.textContent = action.label
      b.addEventListener('click', () => { clearTimeout(toastTimer); t.classList.remove('show'); t.hidden = true; fire('toast'); action.run() })
      t.appendChild(b)
    }
    clearTimeout(toastTimer); clearTimeout(toastHide)
    toastTimer = setTimeout(() => { t.classList.remove('show'); toastHide = setTimeout(() => { t.hidden = true; fire('toast') }, 200) }, action ? 8000 : Math.max(1800, msg.length * 55))   // long enough to read
    fire('toast')
  }
  // ---------- boards ----------
  // Switch the page to another board: the open one is kept in the store, the model takes the new one.
  App.switchBoard = (id) => {
    if (id === store.boardId) return true
    const row = store.boards().find((b) => b.id === id)
    if (!row) return false   // nothing to switch to: leave the page as it is
    App.endEdit(true); model.cancelPreview(); drag = null
    if (App.presenting) fire('present-key', { key: 'Escape' })
    const got = store.open(id)
    if (!got) return false
    App.select([], { raw: true })
    model.reset(got.board)
    for (const b of got.later) model.applyRemote(b)
    loaded.boardId = id
    R.all(); App.countStickies(); fire('board', id)
    App.fitAll(true)
    return true
  }
  App.newBoard = (name, project) => {
    const id = store.create(name, project)
    App.switchBoard(id)
    model.apply([{ op: 'meta', f: { title: name } }], { noUndo: true })
    return id
  }

  App.rect = rect
  App.capture = capture
  App.setDrag = (d) => { drag = d }
})(window)
