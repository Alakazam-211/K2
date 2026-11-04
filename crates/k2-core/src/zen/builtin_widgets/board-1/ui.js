// Board UI: toolbars, the selection toolbar, panels, minimap, presentation,
// timer, voting, comments, theme and boot. Uses BoardApp (app.js).
(function (g) {
  'use strict'
  const App = g.BoardApp; const S = App.S; const G = App.G; const R = App.R; const model = App.model
  const icon = g.BoardIcons.icon
  const host = g.BoardHost
  // The Garden-only parts (a Board file tab turns them off): the board switcher and projects, the
  // "For agents" panel, and the Chat column steering.
  const F = Object.assign({ boards: false, forAgents: false, chat: false }, host.features || {})
  const $ = (id) => document.getElementById(id)
  const mk = (tag, cls, parent, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; if (parent) parent.appendChild(e); return e }

  function btn(parent, name, title, onClick, opts) {
    const b = mk('button', 'btn' + (opts && opts.cls ? ' ' + opts.cls : ''), parent)
    b.type = 'button'; b.title = title; b.setAttribute('aria-label', title)
    if (name) b.appendChild(icon(name, (opts && opts.size) || 20))
    if (opts && opts.label) mk('span', 'lbl', b, opts.label)
    b.addEventListener('click', (e) => { e.stopPropagation(); onClick(b, e) })
    b.addEventListener('pointerdown', (e) => e.stopPropagation())
    return b
  }
  const sep = (parent) => mk('div', 'sep', parent)

  // ---------- popover ----------
  const pop = $('pop'); let popAnchor = null; let popClose = null; let popByPointer = false
  function openPop(anchor, build, opts) {
    if (popAnchor === anchor && !pop.hidden && !(opts && opts.keep)) { closePop(); return }
    closePop()
    pop.textContent = ''; pop.className = 'pop' + (opts && opts.cls ? ' ' + opts.cls : '')
    popAnchor = anchor; popClose = (opts && opts.onClose) || null
    popByPointer = !!(App.pointerJustNow && App.pointerJustNow())   // a click opened it (⌘F or Enter didn't)
    build(pop)
    pop.hidden = false
    placePop(anchor, opts && opts.side, opts && opts.avoid)
    const h = pop.querySelector('.pop-h'); pop.setAttribute('role', opts && opts.menu ? 'menu' : 'dialog'); pop.setAttribute('aria-label', opts && opts.label ? opts.label : h ? h.textContent : (anchor && anchor.getAttribute && anchor.getAttribute('aria-label')) || 'Panel')
    if (anchor && anchor.setAttribute) { anchor.setAttribute('aria-haspopup', opts && opts.menu ? 'menu' : 'dialog'); anchor.setAttribute('aria-expanded', 'true') }
    let af = pop.querySelector('[data-autofocus]')
    // Opened from the keyboard: focus goes into the panel (its first control), not left on the opener.
    // A menu the mouse opened: focus on the menu itself (no ring); the first arrow key goes to its first item.
    if (!af && popByPointer && opts && opts.menu) { pop.tabIndex = -1; pop.focus({ preventScroll: true }) }
    if (!af && !popByPointer) af = pop.querySelector(opts && opts.menu ? '[role^=menuitem][aria-checked=true],[role^=menuitem]' : 'button:not([disabled]),select,input,textarea,[tabindex="0"]')
    if (af) { af.focus({ preventScroll: true }); setTimeout(() => { if (af.isConnected && !pop.contains(document.activeElement)) af.focus({ preventScroll: true }) }, 0) }
    requestAnimationFrame(() => pop.classList.add('show'))
    anchor && anchor.classList && anchor.classList.add('open')
    shareHits()
  }
  // The selection's box on screen (for popovers that shouldn't cover what they edit).
  function selRect() {
    const b = App.selBox(); if (!b) return null
    const [x1, y1] = App.toScreen(b.x, b.y); const [x2, y2] = App.toScreen(b.x + b.w, b.y + b.h)
    return { left: Math.min(x1, x2), top: Math.min(y1, y2), right: Math.max(x1, x2), bottom: Math.max(y1, y2) }
  }
  // avoid: a screen rect the popover shouldn't cover (the object it edits): the first side that misses it wins.
  function placePop(anchor, side, avoid) {
    const r = anchor.getBoundingClientRect ? anchor.getBoundingClientRect() : anchor
    const pw = pop.offsetWidth; const ph = pop.offsetHeight; const W = innerWidth; const H = innerHeight
    const at = (sd) => {
      let x; let y
      if (sd === 'right') { x = r.right + 10; y = r.top + r.height / 2 - ph / 2 }
      else if (sd === 'left') { x = r.left - pw - 10; y = r.top }
      else if (sd === 'point') { x = r.left + pw + 4 > W - 12 ? r.left - pw - 2 : r.left + 2; y = r.top + ph + 4 > H - 12 ? r.top - ph - 2 : r.top + 2 }   // a right-click menu: at the pointer
      else if (sd === 'up') { x = r.left + r.width / 2 - pw / 2; y = r.top - ph - 10 }
      else { x = r.left + r.width / 2 - pw / 2; y = r.bottom + 10 }
      return [Math.max(12, Math.min(W - pw - 12, x)), Math.max(60, Math.min(H - ph - 12, y))]
    }
    let [x, y] = at(side)
    if (avoid) {
      const hits = ([px, py]) => Math.max(0, Math.min(px + pw, avoid.right) - Math.max(px, avoid.left)) * Math.max(0, Math.min(py + ph, avoid.bottom) - Math.max(py, avoid.top))
      const best = [side, 'up', 'down', 'right', 'left'].map((sd) => at(sd)).sort((a, b) => hits(a) - hits(b))[0]
      x = best[0]; y = best[1]
      // Still over it (no side fits): beside it, on whichever side has more room.
      if (hits([x, y]) > 0) {
        const bx = avoid.right + 12 + pw <= W - 12 ? avoid.right + 12 : Math.max(12, avoid.left - pw - 12); const by = Math.max(60, Math.min(H - ph - 12, avoid.top))
        if (hits([bx, by]) < hits([x, y])) { x = bx; y = by }   // only if it really covers less
      }
    }
    pop.style.left = x + 'px'; pop.style.top = y + 'px'
  }
  function closePop() {
    if (pop.hidden) return
    const f = popClose; popClose = null
    pop.hidden = true; pop.classList.remove('show')
    if (popAnchor && popAnchor.classList) { popAnchor.classList.remove('open'); if (popAnchor.setAttribute) popAnchor.setAttribute('aria-expanded', 'false') }
    popAnchor = null
    if (f) f()
    shareHits()
  }
  document.addEventListener('pointerdown', (e) => { if (!pop.hidden && !pop.contains(e.target) && !(popAnchor && popAnchor.contains && popAnchor.contains(e.target))) closePop() }, true)
  App.on('escape', closePop)
  pop.addEventListener('pointerdown', (e) => e.stopPropagation())
  // Escape closes and gives focus back to what opened it (the canvas, when that's gone).
  pop.addEventListener('keydown', (e) => {
    e.stopPropagation()
    if (pop.getAttribute('role') === 'menu' && /^(ArrowDown|ArrowUp|Home|End)$/.test(e.key)) {
      e.preventDefault(); const bs = [...pop.querySelectorAll('[role^=menuitem]')]; const i = bs.indexOf(document.activeElement)
      const n = e.key === 'Home' ? 0 : e.key === 'End' ? bs.length - 1 : i < 0 ? (e.key === 'ArrowDown' ? 0 : bs.length - 1) : (i + (e.key === 'ArrowDown' ? 1 : bs.length - 1)) % bs.length   // from the menu itself: first or last
      if (bs[n]) bs[n].focus()
    }
    if (e.key === 'Escape') { const a = popAnchor; const byPointer = popByPointer; closePop(); (a && a.isConnected && a.focus ? a : $('viewport')).focus(); if (byPointer && App.markPointerFocus) App.markPointerFocus() } })
  pop.addEventListener('wheel', (e) => e.stopPropagation(), { passive: true })

  // ---------- colour swatches ----------
  const swatchCss = (kind, v) => (kind === 'sticky' ? R.colors.stickyColor(v) : kind === 'fill' ? R.colors.fill(v) : R.colors.ink(v))
  function swatches(parent, kind, current, onPick) {
    const names = kind === 'sticky' ? Object.keys(S.STICKY) : kind === 'fill' ? ['none'].concat(Object.keys(S.FILL)) : Object.keys(S.INK)
    const grid = mk('div', 'swatches' + (kind === 'sticky' ? ' big' : ''), parent)
    for (const n of names) {
      const b = mk('button', 'swt' + (n === current ? ' on' : '') + (n === 'none' ? ' none' : ''), grid)
      b.type = 'button'; b.title = n; b.style.setProperty('--c', swatchCss(kind, n))
      b.setAttribute('aria-pressed', String(n === current))
      b.addEventListener('click', () => { onPick(n); grid.querySelectorAll('.swt').forEach((x) => { x.classList.toggle('on', x === b); x.setAttribute('aria-pressed', String(x === b)) }) })
    }
    return grid
  }
  function swatchBtn(parent, kind, current, title, onPick) {
    const b = mk('button', 'btn swbtn', parent); b.type = 'button'; b.title = title
    const dot = mk('span', 'dot' + (current === 'none' ? ' none' : '') + (kind === 'ink' ? ' ring' : ''), b); dot.style.setProperty('--c', swatchCss(kind, current))
    b.addEventListener('pointerdown', (e) => e.stopPropagation())
    b.addEventListener('click', (e) => { e.stopPropagation(); openPop(b, (p) => { mk('div', 'pop-h', p, title); swatches(p, kind, current, (n) => { onPick(n); dot.style.setProperty('--c', swatchCss(kind, n)); dot.classList.toggle('none', n === 'none') }) }) })
    return b
  }
  function seg(parent, options, current, onPick, title) {
    const wrap = mk('div', 'seg', parent); if (title) wrap.title = title
    for (const [v, label, ic] of options) {
      const b = mk('button', 'segb' + (v === current ? ' on' : ''), wrap); b.type = 'button'; b.title = label
      if (ic) b.appendChild(icon(ic, 16)); else b.textContent = label
      b.addEventListener('pointerdown', (e) => e.stopPropagation())
      b.addEventListener('click', (e) => { e.stopPropagation(); wrap.querySelectorAll('.segb').forEach((x) => { x.classList.toggle('on', x === b); x.setAttribute('aria-pressed', String(x === b)) }); onPick(v) })
    }
    return wrap
  }

  // ---------- left toolbar ----------
  const toolbar = $('toolbar'); const toolBtns = {}
  function toolBtn(tool, name, title, opts) {
    const b = btn(toolbar, name, title, (el) => {
      if (opts && opts.popup && App.tool === tool) { opts.popup(el); return }
      App.setTool(tool, opts && opts.toolOpts ? opts.toolOpts() : undefined)
      if (opts && opts.popupFirst) opts.popup(el)
    })
    toolBtns[tool] = b
  }
  App.lastSticky = 'yellow'; App.lastShape = 'round'
  toolBtn('select', 'select', 'Select (V)')
  toolBtn('hand', 'hand', 'Hand (H, or hold Space)')
  sep(toolbar)
  toolBtn('sticky', 'sticky', 'Sticky note (N) — click again for colours', {
    toolOpts: () => ({ fields: { color: App.lastSticky } }),
    popup: (el) => openPop(el, (p) => { mk('div', 'pop-h', p, 'Sticky colour'); swatches(p, 'sticky', App.lastSticky, (n) => { App.lastSticky = n; App.setTool('sticky', { fields: { color: n } }); updateToolDots(); closePop() }) }, { side: 'right' }),
  })
  toolBtn('text', 'text', 'Text (T)')
  toolBtn('shape', 'shape', 'Shapes (S)', {
    toolOpts: () => ({ fields: { shape: App.lastShape } }), popupFirst: true,
    popup: (el) => openPop(el, (p) => shapeGrid(p, App.lastShape, (sh) => { App.lastShape = sh; App.setTool('shape', { fields: { shape: sh } }); closePop() }), { side: 'right' }),
  })
  toolBtn('line', 'line', 'Connection line (L) — drag from one object to another')
  toolBtn('pen', 'pen', 'Pen (P) — click again for colour', {
    toolOpts: () => ({ stroke: App.penColor || 'ink', width: App.penWidth || 3 }),
    popup: (el) => openPop(el, (p) => penPop(p, false), { side: 'right' }),
  })
  toolBtn('highlighter', 'highlighter', 'Highlighter (Shift+P)', {
    toolOpts: () => ({ stroke: App.hlColor || 'yellow', width: 18 }),
    popup: (el) => openPop(el, (p) => penPop(p, true), { side: 'right' }),
  })
  toolBtn('eraser', 'eraser', 'Eraser (E) — erases pen strokes')
  sep(toolbar)
  toolBtn('frame', 'frame', 'Frame (F)')
  toolBtn('card', 'card', 'Card (K)')
  toolBtn('table', 'table', 'Table')
  toolBtn('mind', 'mind', 'Mind map (M)')
  toolBtn('comment', 'comment', 'Comment (C)')
  sep(toolbar)
  btn(toolbar, 'template', 'Templates', (el) => openPop(el, templatesPop, { side: 'right', cls: 'wide' }))
  function penPop(p, hl) {
    mk('div', 'pop-h', p, hl ? 'Highlighter' : 'Pen')
    swatches(p, 'ink', hl ? (App.hlColor || 'yellow') : (App.penColor || 'ink'), (n) => {
      if (hl) App.hlColor = n; else App.penColor = n
      App.setTool(hl ? 'highlighter' : 'pen', { stroke: n, width: hl ? 18 : (App.penWidth || 3) }); updateToolDots()
    })
    if (!hl) {
      mk('div', 'pop-h', p, 'Thickness')
      seg(p, [[2, 'Fine'], [3, 'Medium'], [6, 'Bold'], [12, 'Marker']], App.penWidth || 3, (w) => { App.penWidth = w; App.setTool('pen', { stroke: App.penColor || 'ink', width: w }) })
    }
  }
  function updateToolDots() {
    const set = (tool, css) => { const b = toolBtns[tool]; let d = b.querySelector('.tdot'); if (!d) d = mk('span', 'tdot', b); d.style.setProperty('--c', css) }
    set('sticky', R.colors.stickyColor(App.lastSticky)); set('pen', R.colors.ink(App.penColor || 'ink')); set('highlighter', R.colors.ink(App.hlColor || 'yellow'))
  }
  App.on('tool', () => {
    for (const [t, b] of Object.entries(toolBtns)) { b.classList.toggle('on', App.tool === t); b.setAttribute('aria-pressed', String(App.tool === t)) }
    $('viewport').dataset.tool = App.tool
    zoomBtns.snap.classList.toggle('on', App.snap); zoomBtns.snap.setAttribute('aria-pressed', String(!!App.snap))
  })

  const SHAPE_NAMES = { rect: 'Rectangle', round: 'Rounded rectangle', ellipse: 'Circle', triangle: 'Triangle', diamond: 'Diamond', parallelogram: 'Parallelogram', trapezoid: 'Trapezoid', pentagon: 'Pentagon', hexagon: 'Hexagon', octagon: 'Octagon', star: 'Star', cross: 'Cross', 'arrow-right': 'Arrow right', 'arrow-left': 'Arrow left', chevron: 'Chevron', cylinder: 'Cylinder', cloud: 'Cloud', speech: 'Speech bubble', document: 'Document', 'flow-start': 'Start / end', 'flow-process': 'Process', 'flow-decision': 'Decision', 'flow-data': 'Data', 'flow-predefined': 'Predefined process' }
  function shapeGrid(p, current, onPick) {
    mk('div', 'pop-h', p, 'Shapes')
    const grid = mk('div', 'shapegrid', p)
    for (const sh of S.SHAPES) {
      const b = mk('button', 'shp' + (sh === current ? ' on' : ''), grid); b.type = 'button'; b.title = SHAPE_NAMES[sh] || sh
      const NS = 'http://www.w3.org/2000/svg'; const svg = document.createElementNS(NS, 'svg')
      const w = ['arrow-right', 'arrow-left', 'chevron', 'flow-start', 'flow-process', 'flow-data', 'flow-predefined', 'parallelogram', 'trapezoid', 'document', 'speech'].includes(sh) ? 26 : 22
      const h = ['flow-start'].includes(sh) ? 12 : ['arrow-right', 'arrow-left', 'chevron', 'flow-process', 'flow-data', 'flow-predefined', 'parallelogram', 'trapezoid', 'document', 'speech', 'cylinder'].includes(sh) ? 17 : 22
      svg.setAttribute('viewBox', (-(28 - w) / 2) + ' ' + (-(28 - h) / 2) + ' 28 28'); svg.setAttribute('width', 28); svg.setAttribute('height', 28)
      const path = document.createElementNS(NS, 'path'); path.setAttribute('d', G.shapePath(sh, w, h)); svg.appendChild(path); b.appendChild(svg)
      b.addEventListener('click', () => onPick(sh))
    }
  }

  // ---------- top bars ----------
  let hint = null   // the empty-board hint
  const topleft = $('topleft')
  const nameEl = mk('div', 'boardname', topleft)
  mk('span', 'logo', nameEl).appendChild(icon('frame', 16))
  const nameBtn = mk('button', 'name', nameEl); nameBtn.type = 'button'; nameBtn.title = 'Boards (double-click to rename)'
  const nameText = mk('span', '', nameBtn); nameBtn.appendChild(icon('chevron', 14))
  if (F.boards) nameBtn.addEventListener('click', () => openPop(nameBtn, boardsPop, { cls: 'wide boards', side: 'down' }))
  else { nameBtn.title = 'Rename the board'; nameBtn.addEventListener('click', () => renameBoard()) }
  nameBtn.addEventListener('dblclick', (e) => { e.preventDefault(); closePop(); renameBoard() })
  nameBtn.addEventListener('pointerdown', (e) => e.stopPropagation())
  const statusEl = mk('span', 'status', nameEl)
  sep(topleft)
  const undoBtn = btn(topleft, 'undo', 'Undo (⌘Z)', () => model.undo())
  const redoBtn = btn(topleft, 'redo', 'Redo (⇧⌘Z)', () => model.redo())
  function boardName() {
    const b = boardRows().find((x) => x.id === App.store.boardId)
    return model.meta.title || (b && b.name) || 'Board'
  }
  function drawName() { nameText.textContent = boardName() }
  App.store.onStatus((st) => {
    statusEl.textContent = st.text; statusEl.dataset.state = st.state
    statusEl.title = st.state === 'error' ? st.text : (st.detail || '')
  })

  // ---------- boards: switch, create, rename ----------
  // Every board the switcher can open, from every source (store.sources: this Garden's data today; board
  // files in other workspaces later, with a workspace badge). Rows keep the switcher's own field names.
  const boardRows = () => App.store.sources.flatMap((src) => src.list().map((b) => Object.assign({}, b, { source: src.id, name: b.title })))
  // Board's own projects: names by id.
  const projectNames = () => { const out = {}; for (const x of App.store.projects()) out[x.id] = x.name; return out }
  const ownProject = (k) => App.store.projects().some((x) => x.id === k)
  // An inline name field in the popover: Enter keeps it, Escape or an empty name drops it.
  const NAME_MAX = 200   // board and project names, as the title and the k2 board CLI take them
  function nameField(parent, value, placeholder, done, before) {
    const input = mk('input', 'field rename'); input.value = value || ''; input.placeholder = placeholder
    input.setAttribute('aria-label', placeholder); input.dataset.autofocus = '1'
    if (before) parent.insertBefore(input, before); else parent.appendChild(input)
    let over = false
    const finish = (keep) => {
      if (over) return; over = true
      const v = input.value.replace(/\s+/g, ' ').trim()
      if (keep && S.chars(v) > NAME_MAX) { App.toast(App.refusalText(S.refusal('too_long', 'name', NAME_MAX))); done(null); return }   // refused, never cut
      done(keep && v ? v : null)
    }
    input.addEventListener('keydown', (e) => { e.stopPropagation(); if (e.key === 'Enter') { e.preventDefault(); finish(true) } else if (e.key === 'Escape') { e.preventDefault(); finish(false) } })
    input.addEventListener('blur', () => finish(true))
    input.focus({ preventScroll: true })
    return input
  }
  const moveResult = (b, to, pr) => pr.then(() => App.toast('Moved “' + (b.name || 'Board') + '” to ' + (to ? projectNames()[to] || 'the project' : 'No project')))
    .catch((e) => App.toast((e && (e.code === 'busy' || e.code === 'not_saved')) ? 'Moving “' + (b.name || 'Board') + '”: it will finish once saving catches up.' : 'Couldn’t move “' + (b.name || 'Board') + '”: try again'))

  // Board files (a preview): every .k5board the person may read, in any workspace on this computer, by name. K2
  // lists them and says when they change; a click opens the file in its own Board tab. They sit beside this Garden's
  // boards in the switcher, never mixed in: nothing is copied or moved.
  const files = { on: false, rows: null, truncated: false, failed: null }
  let filesShown = null   // the open switcher's {tree, draw}, while it shows
  const fileRows = () => (files.rows || []).slice().sort((a, b) => a.workspaceName.localeCompare(b.workspaceName) || a.rel.localeCompare(b.rel))
  // A new list redraws the open switcher, unless a name is being typed in it (the next open shows the list).
  function filesChanged() {
    const sw = filesShown
    if (!sw || !sw.tree.isConnected || pop.hidden || popAnchor !== nameBtn || sw.tree.querySelector('input.rename')) return
    sw.draw()
  }
  function openFile(f) {
    host.openFile(f.id).then(
      (ok) => { if (!ok) App.toast('Couldn’t open “' + f.name + '”: its workspace isn’t in this window') },
      (e) => App.toast('Couldn’t open “' + f.name + '” (' + ((e && e.code) || 'error') + ')'))
  }

  // The board list: one page, a file tree. Each project a folder (open unless closed on this computer), its
  // boards under it, "No project" last, the Trash at the bottom. Every row has the same menu, by right-click or
  // ⋯ (Rename, New board here, Delete); double-click renames; drag a board onto a folder to move it.
  const DAY = 864e5
  function boardsPop(p) {
    const all = boardRows(); const cur = App.store.boardId; const names = projectNames()
    const own = true
    const projAll = own ? App.store.projects() : []
    const trashedP = new Set(projAll.filter((x) => x.trashedAt).map((x) => x.id))
    const list = all.filter((b) => !b.trashedAt)
    const closed = new Set(Array.isArray(App.store.prefs().treeClosed) ? App.store.prefs().treeClosed : ['trash'])
    const redraw = () => { p.textContent = ''; boardsPop(p); placePop(nameBtn, 'down'); shareHits() }
    const head = mk('div', 'thead', p); mk('div', 'pop-h', head, 'Boards')
    if (own) {
      const np = mk('button', 'tnew', head); np.type = 'button'; np.appendChild(icon('plus', 14)); mk('span', '', np, 'New project')
      np.addEventListener('click', () => { np.hidden = true; nameField(tree, '', 'Project name', (v) => { if (v) App.store.createProject(v); redraw() }, tree.firstChild) })
    }
    let input = null
    if (list.length + (files.on && files.rows ? files.rows.length : 0) > 8) { input = mk('input', 'field search', p); input.placeholder = 'Find a board'; input.dataset.autofocus = '1' }
    const tree = mk('div', 'blist tree', p); tree.setAttribute('role', 'tree')
    let menu = null
    const closeMenu = () => { if (menu) { menu.remove(); menu = null } }
    p.addEventListener('pointerdown', (e) => { if (menu && !menu.contains(e.target)) closeMenu() })
    // The row's menu, opened under the row (right-click or ⋯). items: [label, run] or a confirm step.
    function rowMenu(row, items, at) {
      closeMenu()
      menu = mk('div', 'rowmenu', p); menu.setAttribute('role', 'menu')
      const fill = (list2) => {
        menu.textContent = ''
        for (const it of list2) {
          if (it.text) { mk('div', 'rmtext', menu, it.text); continue }
          if (it.editor) { menu.classList.add('wide'); it.editor(mk('div', 'aedit', menu)); continue }
          const b = mk('button', 'menuitem' + (it.danger ? ' danger' : ''), menu); b.type = 'button'; b.setAttribute('role', 'menuitem')
          if (it.icon) b.appendChild(icon(it.icon, 16)); mk('span', '', b, it.label)
          b.addEventListener('click', (e) => { e.stopPropagation(); const next = it.run(); if (Array.isArray(next)) fill(next); else closeMenu() })
        }
        const pr = p.getBoundingClientRect(); const rr = row.getBoundingClientRect()
        menu.style.top = Math.min(rr.bottom - pr.top + 2, p.clientHeight - menu.offsetHeight - 6) + 'px'
        menu.style.left = Math.max(6, Math.min((at ? at.x - pr.left : rr.right - pr.left - 190), p.clientWidth - 196)) + 'px'
        const f = menu.querySelector('button'); if (f) f.focus({ preventScroll: true })
      }
      fill(items)
      menu.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') { e.stopPropagation(); closeMenu(); if (row.isConnected) row.focus() }
        else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); const bs = [...menu.querySelectorAll('button')]; const i = bs.indexOf(document.activeElement); const n = bs[(i + (e.key === 'ArrowDown' ? 1 : bs.length - 1)) % bs.length]; if (n) n.focus() }
      })
    }
    const withMenu = (row, items) => {
      const more = mk('span', 'bmore', row, '⋯'); more.setAttribute('role', 'button'); more.title = 'More'; more.setAttribute('aria-label', 'More'); more.tabIndex = 0
      more.addEventListener('click', (e) => { e.stopPropagation(); rowMenu(row, items()) })
      more.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); e.stopPropagation(); rowMenu(row, items()) } })
      row.addEventListener('contextmenu', (e) => { e.preventDefault(); e.stopPropagation(); rowMenu(row, items(), { x: e.clientX }) })
    }
    // Delete: say exactly what goes, then put it in the Trash with Undo. Any board can go; when the last one
    // does, a new empty board takes its place.
    const del = (kind, id, label, count) => [
      { text: kind === 'project' ? 'Delete project “' + label + '”' + (count ? ' and its ' + count + ' board' + (count === 1 ? '' : 's') : '') + '?' : 'Delete board “' + label + '”?' },
      { text: 'It goes to the Trash for 30 days.' },
      { label: 'Delete', icon: 'trash', danger: true, run: () => {
        const pr = App.store.trash(kind, id)
        const gone = kind === 'project' ? boardRows().filter((b) => b.trashedWith === id).map((b) => b.id) : [id]
        if (gone.includes(App.store.boardId)) { const next = boardRows().find((b) => !b.trashedAt); if (next) App.switchBoard(next.id) }
        App.toast('Deleted “' + label + '”', { label: 'Undo', run: () => App.store.restore(kind, id).then((r) => App.toast(r.saved ? 'Restored “' + label + '”' : 'Restoring “' + label + '”: it finishes once saving catches up')) })
        closePop()
        // Say so if it didn't fully happen (not saved yet).
        pr.then((r) => { if (!r.saved) App.toast(r.why ? 'Not deleted yet: ' + r.why : 'Deleting “' + label + '”: it finishes once saving catches up') })
      } },
      { label: 'Cancel', run: () => {} },
    ]
    const renameRow = (row, value, placeholder, save) => { closeMenu(); row.hidden = true; nameField(row.parentElement, value, placeholder, (v) => { if (v && v !== value) save(v); redraw() }, row) }
    let dragged = null   // {kind: 'board' | 'project', id, project | parent}
    // Where a drop lands on a row: the top third of a folder or half of a board = before it; else into the folder / after the board.
    const zone = (e, row, folder) => { const r = row.getBoundingClientRect(); const y = (e.clientY - r.top) / r.height; return folder ? (y < 0.3 ? 'before' : 'into') : (y < 0.5 ? 'before' : 'after') }
    const clearDrop = () => tree.querySelectorAll('.drop, .dropbefore, .dropafter').forEach((x) => x.classList.remove('drop', 'dropbefore', 'dropafter'))
    const live = (k) => own && ownProject(k) && !trashedP.has(k)
    const projOf = (k) => projAll.find((x) => x.id === k) || null
    // A loop (two moves at once) or a gone parent: shown at the top level, as the agent CLI treats it.
    // Any loop on the way up (even above it) makes it top level, exactly as the store and the agent CLI count it.
    const parentOf = (k) => { const x = projOf(k); if (!x || !x.parent || !live(x.parent)) return null; const seen = [k]; let q = x.parent; while (q) { if (seen.includes(q) || seen.length > 64) return null; seen.push(q); const y = projOf(q); q = y && y.parent && projOf(y.parent) ? y.parent : null } return x.parent }
    const inSubtree = (k, root) => { let q = k; for (let n = 0; q && n < 64; n++) { if (q === root) return true; q = parentOf(q) } return false }
    const byOrder = (a, b) => (a.order ?? 1e9) - (b.order ?? 1e9) || (a.name || '').localeCompare(b.name || '')
    const kidProjects = (k) => projAll.filter((x) => live(x.id) && (parentOf(x.id) || null) === (k || null)).sort(byOrder)
    const boardsIn = (k) => list.filter((b) => (b.project || '') === (k || '')).sort(byOrder)
    // Moves and reorders from a drop. Boards: into a folder (end), or before/after a board. Projects: into a folder, or before a folder.
    function dropOn(target, where) {
      const d = dragged; dragged = null; clearDrop(); if (!d) return
      if (d.kind === 'board') {
        const to = target.kind === 'folder' ? target.id : (target.board.project || null)
        const sib = boardsIn(to || '').filter((b) => b.id !== d.id).map((b) => b.id)
        let at = sib.length
        if (target.kind === 'board') { at = sib.indexOf(target.board.id); if (where === 'after') at++ }
        sib.splice(Math.max(0, at), 0, d.id)
        const b = list.find((x) => x.id === d.id)
        const done = () => { App.store.reorder('board', sib); redraw() }
        if ((b.project || null) !== (to || null)) { closePop(); moveResult(b, to, App.store.move(b.id, to).then((r) => { App.store.reorder('board', sib); return r })) } else done()
        return
      }
      // A project: never into itself or its own subtree.
      const parent = where === 'into' ? (target.id || null) : parentOf(target.id)
      if (parent && inSubtree(parent, d.id)) { App.toast('A project can’t go inside itself.'); return }
      const sib = kidProjects(parent).filter((x) => x.id !== d.id).map((x) => x.id)
      let at = sib.length; if (where === 'before') at = Math.max(0, sib.indexOf(target.id))
      sib.splice(at, 0, d.id)
      App.store.nest(d.id, parent).then((r) => { if (r.refused) App.toast('Couldn’t move that project there.'); else if (!r.saved) App.toast('Moving the project: it finishes once saving catches up') })
      App.store.reorder('project', sib); redraw()
    }
    // One board's row (in a folder, or in search results).
    function boardRow(box, b) {
      const row = mk('button', 'menuitem brow trow' + (b.id === cur ? ' on' : ''), box); row.type = 'button'; row.setAttribute('role', 'treeitem'); row.dataset.tkey = 'b:' + b.id
      if (b.id === cur) row.setAttribute('aria-current', 'true')
      const label = b.id === cur ? boardName() : (b.name || 'Untitled board')
      mk('span', 'tname', row, label)
      if (b.workspace) { const ws = mk('kbd', 'wsbadge', row, String(b.workspace.label || b.workspace.id || b.workspace)); ws.title = 'In ' + ws.textContent }   // a board file from a workspace
      if (b.agentEdit && b.id !== cur) { mk('i', 'bdot', row); row.title = b.agentEdit + ' changed this board' }
      const rename = () => { if (b.id === cur) { closePop(); renameBoard() } else renameRow(row, b.name || '', 'Board name', (v) => App.store.rename(b.id, v)) }
      if (own) {
        withMenu(row, () => [
          { label: 'Rename', icon: 'text', run: rename },
          { label: 'Delete', icon: 'trash', danger: true, run: () => del('board', b.id, label) },
        ])
        row.draggable = true
        row.addEventListener('dragstart', (e) => { dragged = { kind: 'board', id: b.id }; try { e.dataTransfer.setData('text/plain', b.id); e.dataTransfer.effectAllowed = 'move' } catch (_) {} row.classList.add('dragging') })
        row.addEventListener('dragend', () => { dragged = null; row.classList.remove('dragging'); clearDrop() })
        row.addEventListener('dragover', (e) => { if (!dragged || dragged.kind !== 'board' || dragged.id === b.id || (b.project && !live(b.project))) return; e.preventDefault(); clearDrop(); row.classList.add(zone(e, row, false) === 'before' ? 'dropbefore' : 'dropafter') })
        row.addEventListener('dragleave', () => row.classList.remove('dropbefore', 'dropafter'))
        row.addEventListener('drop', (e) => { e.preventDefault(); e.stopPropagation(); dropOn({ kind: 'board', board: b }, zone(e, row, false)) })
      }
      let clicks = null
      row.addEventListener('click', (e) => {
        if (e.target.closest('.bmore')) return
        // A double-click renames: wait a moment before opening, so the second click can cancel it.
        clearTimeout(clicks)
        clicks = setTimeout(() => {
          closePop(); if (b.id === cur) return
          if (!App.switchBoard(b.id)) App.toast('Couldn’t open “' + (b.name || 'the board') + '”')
        }, 220)
      })
      row.addEventListener('dblclick', (e) => {
        e.preventDefault(); clearTimeout(clicks)
        rename()
      })
      return row
    }
    // One board file's row: its name and workspace (its path on hover). A click opens its tab; no menu, no drag.
    function fileRow(box, f) {
      const row = mk('button', 'menuitem brow trow frow', box); row.type = 'button'; row.setAttribute('role', 'treeitem'); row.dataset.tkey = 'f:' + f.id
      mk('span', 'tname', row, f.name || 'Untitled board')
      const ws = mk('kbd', 'wsbadge', row, f.workspaceName); ws.title = 'In ' + f.workspaceName
      row.title = f.rel
      row.addEventListener('click', () => { closePop(); openFile(f) })
      return row
    }
    // The Board files folder, after this Garden's own boards (closed and open like any folder).
    function filesFolder() {
      const fopen = !closed.has('files')
      const ff = mk('div', 'tfolder tfiles' + (fopen ? ' open' : ''), tree); ff.setAttribute('role', 'none')
      const fr = mk('div', 'trow tfold', ff); fr.setAttribute('role', 'treeitem'); fr.setAttribute('aria-expanded', String(fopen)); fr.setAttribute('aria-label', 'Board files'); fr.dataset.tkey = 'f:'
      const ch = mk('button', 'tchev', fr); ch.type = 'button'; ch.appendChild(icon('chevron', 14)); ch.setAttribute('aria-label', (fopen ? 'Close' : 'Open') + ' Board files')
      fr.appendChild(icon('folder', 16)); mk('span', 'tname', fr, 'Board files')
      if (files.rows) mk('span', 'tcount', fr, String(files.rows.length))
      fr.addEventListener('click', () => { if (closed.has('files')) closed.delete('files'); else closed.add('files'); App.store.setPref('treeClosed', [...closed]); draw() })
      if (!fopen) return
      const box = mk('div', 'tkids', ff); box.setAttribute('role', 'group')
      if (files.failed) { mk('div', 'tempty', box, 'Couldn’t list board files (' + files.failed + ')'); return }
      if (!files.rows) { mk('div', 'tempty', box, 'Looking for board files…'); return }
      if (!files.rows.length) { mk('div', 'tempty', box, 'No board files in your workspaces'); return }
      for (const f of fileRows()) fileRow(box, f)
      if (files.truncated) mk('div', 'tempty', box, 'Showing the first 2,000 files')
    }
    const draw = () => { const had = tree.contains(document.activeElement); draw0(); rove(had) }
    filesShown = { tree, draw: () => draw() }
    // The tree is one Tab stop; arrow keys move in it (Up/Down rows, Right opens or goes in, Left closes or goes out,
    // Home/End, Enter opens, Shift+F10 or the menu key: the row's menu).
    let focusKey = null
    const rows = () => [...tree.querySelectorAll('.trow')].filter((r) => !r.closest('.tfolder:not(.open) > .tkids'))   // a closed folder's rows don't count
    function rove(had) {
      const all2 = rows()
      for (const r of all2) {
        r.tabIndex = -1
        let n = 0; for (let x = r.parentElement; x && x !== tree; x = x.parentElement) if (x.classList.contains('tfolder')) n++
        r.setAttribute('aria-level', String(Math.max(1, n + (r.classList.contains('tfold') ? 0 : 1))))
        for (const c of r.querySelectorAll('.tchev, .bmore')) c.tabIndex = -1
      }
      const pick = all2.find((r) => r.dataset.tkey === focusKey) || all2.find((r) => r.getAttribute('aria-current')) || all2[0]
      if (pick) { pick.tabIndex = 0; if (had && focusKey && pick.dataset.tkey === focusKey) pick.focus() }   // a redraw under the keyboard keeps its place
    }
    tree.addEventListener('focusin', (e) => { const r = e.target.closest && e.target.closest('.trow'); if (r && r.dataset.tkey) { focusKey = r.dataset.tkey; for (const x of rows()) x.tabIndex = x === r ? 0 : -1 } })
    tree.addEventListener('keydown', (e) => {
      const r = e.target.closest && e.target.closest('.trow'); if (!r || e.target !== r || menu && menu.contains(e.target)) return
      const all2 = rows(); const i = all2.indexOf(r); const isFold = r.classList.contains('tfold')
      const open = isFold && r.getAttribute('aria-expanded') === 'true'
      const go = (x) => { if (x) { e.preventDefault(); x.focus() } }
      const parentRow = () => { const f = r.parentElement.closest('.tfolder'); const own2 = isFold ? f && f.parentElement.closest('.tfolder') : f; return own2 ? own2.querySelector(':scope > .tfold') : null }
      if (e.key === 'ArrowDown') go(all2[i + 1])
      else if (e.key === 'ArrowUp') go(all2[i - 1])
      else if (e.key === 'Home') go(all2[0])
      else if (e.key === 'End') go(all2[all2.length - 1])
      else if (e.key === 'ArrowRight' && isFold) { e.preventDefault(); if (!open) r.click(); else go(all2[i + 1]) }
      else if (e.key === 'ArrowLeft') { e.preventDefault(); if (open) r.click(); else go(parentRow()) }
      else if ((e.key === 'Enter' || e.key === ' ') && !r.matches('button')) { e.preventDefault(); const rb = r.querySelector('.trestore'); if (rb) rb.click(); else r.click() }
      else if (e.key === 'ContextMenu' || (e.shiftKey && e.key === 'F10')) { const m = r.querySelector(':scope > .bmore'); if (m) { e.preventDefault(); m.click() } }
    })
    const draw0 = () => {
      tree.textContent = ''; closeMenu()
      const q = input ? input.value.trim().toLowerCase() : ''
      // Searching: the matching boards, flat, each with its folder path.
      if (q) {
        const hits = list.filter((b) => (b.name || '').toLowerCase().includes(q))
        const fhits = files.on ? fileRows().filter((f) => (f.name || '').toLowerCase().includes(q)) : []
        if (!hits.length && !fhits.length) { mk('div', 'empty', tree, 'No board called that'); return }
        const path = (k) => { const out = []; let x = k; for (let n = 0; x && n < 64; n++) { out.unshift(names[x] || 'Linked project'); x = parentOf(x) } return out.join(' / ') }
        for (const b of hits) { const row = boardRow(tree, b); if (b.project) mk('kbd', 'tpath', row, path(b.project)) }
        for (const f of fhits) fileRow(tree, f)
        return
      }
      const folder = (parent, k, depth) => {
        const isNone = k === ''
        const title = isNone ? 'No project' : names[k] || 'Linked project'
        const kids = isNone ? [] : kidProjects(k); const boards = boardsIn(k)
        const open = !closed.has(k || '-')
        const f = mk('div', 'tfolder' + (open ? ' open' : ''), parent); f.setAttribute('role', 'none')
        const fr = mk('div', 'trow tfold', f); fr.setAttribute('role', 'treeitem'); fr.setAttribute('aria-expanded', String(!!open)); fr.setAttribute('aria-label', title); fr.dataset.tkey = 'p:' + (k || '-')
        const chev = mk('button', 'tchev', fr); chev.type = 'button'; chev.appendChild(icon('chevron', 14)); chev.setAttribute('aria-label', (open ? 'Close ' : 'Open ') + title)
        fr.appendChild(icon('folder', 16)); mk('span', 'tname', fr, title)
        mk('span', 'tcount', fr, String(boards.length))
        const toggle = () => { if (closed.has(k || '-')) closed.delete(k || '-'); else closed.add(k || '-'); App.store.setPref('treeClosed', [...closed]); draw() }
        chev.addEventListener('click', (e) => { e.stopPropagation(); toggle() })
        let fclick = null
        fr.addEventListener('click', (e) => { if (e.target.closest('input, .bmore')) return; clearTimeout(fclick); if (!e.detail) { toggle(); return } fclick = setTimeout(toggle, 220) })   // keyboard (detail 0): at once   // a double-click renames instead
        const canAdd = isNone || live(k)
        const newHere = () => { closePop(); App.newBoard('Untitled board', k || null); renameBoard() }
        const newProjectHere = () => { closeMenu(); nameField(f, '', 'Project name', (v) => { if (v) { App.store.createProject(v, k || null); closed.delete(k || '-') } redraw() }, f.children[1] || null) }
        if (canAdd) {
          const plus = mk('button', 'tplus', fr); plus.type = 'button'; plus.title = 'New board in ' + title; plus.setAttribute('aria-label', plus.title); plus.appendChild(icon('plus', 14))
          plus.addEventListener('click', (e) => { e.stopPropagation(); newHere() })
        }
        if (live(k)) {
          const n = list.filter((b) => { let x = b.project; for (let m = 0; x && m < 64; m++) { if (x === k) return true; x = parentOf(x) } return false }).length   // every live board under it, sub-projects too
          fr.addEventListener('dblclick', (e) => { if (e.target.closest('input')) return; e.stopPropagation(); clearTimeout(fclick); renameRow(fr, names[k], 'Project name', (v) => App.store.renameProject(k, v)) })
          withMenu(fr, () => [
            { label: 'Rename', icon: 'text', run: () => renameRow(fr, names[k], 'Project name', (v) => App.store.renameProject(k, v)) },
            { label: 'New board here', icon: 'plus', run: newHere },
            { label: 'New project here', icon: 'folder', run: newProjectHere },
            { label: 'Delete', icon: 'trash', danger: true, run: () => del('project', k, names[k], n) },
          ])
          fr.draggable = true
          fr.addEventListener('dragstart', (e) => { e.stopPropagation(); dragged = { kind: 'project', id: k }; try { e.dataTransfer.setData('text/plain', k); e.dataTransfer.effectAllowed = 'move' } catch (_) {} fr.classList.add('dragging') })
          fr.addEventListener('dragend', () => { dragged = null; fr.classList.remove('dragging'); clearDrop() })
        } else if (own && isNone) withMenu(fr, () => [{ label: 'New board here', icon: 'plus', run: newHere }, { label: 'New project here', icon: 'folder', run: newProjectHere }])
        // Drops: own folders and "No project" (a linked K2 project isn't a place to move to here).
        if (own && (isNone || live(k))) {
          fr.addEventListener('dragover', (e) => {
            if (!dragged || (dragged.kind === 'project' && (dragged.id === k || (k && inSubtree(k, dragged.id))))) return
            const w = isNone ? 'into' : zone(e, fr, true)
            if (dragged.kind === 'board' && w === 'before') return
            e.preventDefault(); clearDrop(); fr.classList.add(w === 'into' ? 'drop' : 'dropbefore')
          })
          fr.addEventListener('dragleave', () => fr.classList.remove('drop', 'dropbefore'))
          fr.addEventListener('drop', (e) => { e.preventDefault(); e.stopPropagation(); dropOn({ kind: 'folder', id: k || null }, isNone ? 'into' : zone(e, fr, true)) })
        }
        if (!open) return
        const box = mk('div', 'tkids', f); box.setAttribute('role', 'group')
        for (const x of kids) folder(box, x.id, depth + 1)
        if (!kids.length && !boards.length) mk('div', 'tempty', box, 'Empty')
        for (const b of boards) boardRow(box, b)
      }
      // Top level: Board's own projects with no parent, then boards whose project isn't listed, then "No project".
      const tops = kidProjects(null).map((x) => x.id)
      const linked = [...new Set(list.map((b) => b.project).filter((k) => k && !live(k) && !trashedP.has(k)))].sort((a, b) => (names[a] || '').localeCompare(names[b] || ''))
      for (const k of tops.concat(linked)) folder(tree, k, 0)
      folder(tree, '', 0)
      if (files.on) filesFolder()
      // The Trash: what was deleted in the last 30 days, restorable (closed until opened).
      if (!own || q) return
      const tp = projAll.filter((x) => x.trashedAt && !x.trashedWith); const tb = all.filter((b) => b.trashedAt && !(b.trashedWith && trashedP.has(b.trashedWith)))
      if (!tp.length && !tb.length) return
      const topen = !closed.has('trash')
      const tf = mk('div', 'tfolder ttrash' + (topen ? ' open' : ''), tree)
      const tr = mk('div', 'trow tfold', tf); tr.setAttribute('role', 'treeitem'); tr.setAttribute('aria-expanded', String(topen)); tr.setAttribute('aria-label', 'Trash'); tr.dataset.tkey = 't:'
      const tch = mk('button', 'tchev', tr); tch.type = 'button'; tch.appendChild(icon('chevron', 14)); tch.setAttribute('aria-label', topen ? 'Close Trash' : 'Open Trash')
      tr.appendChild(icon('trash', 16)); mk('span', 'tname', tr, 'Trash'); mk('span', 'tcount', tr, String(tp.length + tb.length))
      tr.addEventListener('click', () => { if (closed.has('trash')) closed.delete('trash'); else closed.add('trash'); App.store.setPref('treeClosed', [...closed]); draw() })
      if (!topen) return
      const kids = mk('div', 'tkids', tf); kids.setAttribute('role', 'group')
      const left = (at) => { const d = Date.parse(at); return isNaN(d) ? 30 : Math.max(0, 30 - Math.floor((Date.now() - d) / DAY)) }
      const item = (label, at, kind, id) => {
        const r = mk('div', 'trow tgone', kids); r.setAttribute('role', 'treeitem'); r.setAttribute('aria-label', label + ', in the Trash'); r.dataset.tkey = 'g:' + kind + ':' + id; mk('span', 'tname', r, label); mk('span', 'tcount', r, left(at) + ' days left')
        const rb = mk('button', 'trestore', r, 'Restore'); rb.type = 'button'
        rb.addEventListener('click', (e) => { e.stopPropagation(); App.store.restore(kind, id).then((r) => App.toast(r.saved ? 'Restored “' + label + '”' : 'Restoring “' + label + '”: it finishes once saving catches up')); redraw() })
      }
      for (const x of tp) item(x.name + ' (project, ' + all.filter((b) => b.trashedWith === x.id).length + ' boards)', x.trashedAt, 'project', x.id)
      for (const b of tb) item(b.name || 'Untitled board', b.trashedAt, 'board', b.id)
    }
    if (input) input.addEventListener('input', draw)
    draw()
  }
  function renameBoard() {
    if (nameEl.querySelector('input')) return
    const was = boardName()
    const input = mk('input', 'field rename'); input.value = was; input.setAttribute('aria-label', 'Board name')
    nameBtn.hidden = true; nameEl.insertBefore(input, statusEl)
    input.focus(); input.select(); shareHits()
    let done = false
    const finish = (keep) => {
      if (done) return; done = true
      const v = input.value.replace(/\s+/g, ' ').trim()
      input.remove(); nameBtn.hidden = false
      // The title first: when it's refused (too long, or a board that's read only) the list keeps the old name too.
      if (keep && v && v !== was && model.apply([{ op: 'meta', f: { title: v } }])) App.store.rename(App.store.boardId, v)
      drawName(); shareHits(); $('viewport').focus({ preventScroll: true })
    }
    input.addEventListener('keydown', (e) => { e.stopPropagation(); if (e.key === 'Enter') finish(true); else if (e.key === 'Escape') finish(false) })
    input.addEventListener('blur', () => finish(true))
    input.addEventListener('pointerdown', (e) => e.stopPropagation())
  }
  App.on('board', () => { closePop(); endActivity(); hudKey = ''; drawHud(); R.setVoting(!!(model.meta.voting && model.meta.voting.active)); drawName(); emptyHint(); drawMinimap() })
  const topright = $('topright')
  // Light / dark: Auto follows K2's theme; Light or Dark overrides it and is kept across reloads.
  const SCHEMES = [['auto', 'contrast', 'Auto', 'Follow K2'], ['light', 'sun', 'Light', ''], ['dark', 'moon', 'Dark', '']]
  let pickScheme = SCHEMES.some((x) => x[0] === App.store.prefs().scheme) ? App.store.prefs().scheme : 'auto'
  let hostScheme = null
  const themeNow = () => applyScheme(pickScheme === 'auto' ? hostScheme : pickScheme)
  const themeBtn = btn(topright, 'contrast', 'Light or dark', (el) => openPop(el, themePop, { side: 'down', menu: true }))
  function drawThemeBtn() {
    const s = SCHEMES.find((x) => x[0] === pickScheme)
    themeBtn.textContent = ''; themeBtn.appendChild(icon(s[1], 20))
    themeBtn.title = 'Light or dark: ' + s[2]; themeBtn.setAttribute('aria-label', themeBtn.title)
  }
  function themePop(p) {
    mk('div', 'pop-h', p, 'Appearance')
    for (const [v, ic, label, note] of SCHEMES) {
      const b = mk('button', 'menuitem' + (v === pickScheme ? ' on' : ''), p); b.type = 'button'
      b.setAttribute('role', 'menuitemradio'); b.setAttribute('aria-checked', String(v === pickScheme))
      b.appendChild(icon(ic, 18)); mk('span', '', b, label); if (note) mk('kbd', '', b, note)
      if (v === pickScheme) b.appendChild(icon('check', 16))
      b.addEventListener('click', () => {
        closePop(); if (v === pickScheme) return
        pickScheme = v; themeNow(); drawThemeBtn(); App.store.setPref('scheme', v)
      })
    }
  }
  drawThemeBtn()
  // An agent's picture (K2's image), or its initials on its colour.
  function avatar(parent, a, cls) {
    const el = mk('span', 'av' + (cls ? ' ' + cls : ''), parent)
    if (a && a.image) { const im = document.createElement('img'); im.src = a.image; im.alt = ''; el.appendChild(im); el.classList.add('pic') }
    else { el.textContent = (String((a && (a.label || a.name || a.handle)) || '?').split(/[^A-Za-z0-9]+/).filter(Boolean).slice(0, 2).map((w) => w[0]).join('') || '?').toUpperCase(); if (a && a.color) { el.style.background = a.color; el.style.color = '#fff' } }
    return el
  }
  // Copy a line of text: the clipboard if the frame allows it, else select it for ⌘C.
  function copyText(text, el) {
    const pick = () => { const r = document.createRange(); r.selectNodeContents(el); const s = getSelection(); s.removeAllRanges(); s.addRange(r); App.toast('Press ⌘C to copy the selected text') }
    try { navigator.clipboard.writeText(text).then(() => App.toast('Copied'), pick) } catch (_) { pick() }
  }

  // ---------- For agents: how the person lets an agent work on this board ----------
  // Read-only. Agents reach a board only through K2's own data grants, which the person gives in K2 (a command or
  // Settings), never from inside the board.
  App.grantCommand = (level, id, handle) => 'k2 zen data grant k2:board --agent ' + (handle || '<handle>') + ' --level ' + level + ' --prefix board/' + id + '/'
  function forAgentsPop(p) {
    const id = App.store.boardId
    mk('div', 'pop-h', p, 'For agents')
    mk('div', 'note', p, 'Agents can’t open your boards until you let them. To let one work on “' + boardName() + '”, run one of these in a terminal:')
    const pickRow = mk('label', 'frow', p); mk('span', '', pickRow, 'Agent')
    const who = mk('select', 'field agentsel', pickRow); who.setAttribute('aria-label', 'Agent')
    const o0 = mk('option', '', who, 'Any agent (<handle>)'); o0.value = ''
    const cmds = []
    const line = (label, level) => {
      const r = mk('div', 'cmdrow', p); mk('span', 'cmdlbl', r, label)
      const c = mk('code', 'cmd', r, App.grantCommand(level, id)); c.dataset.level = level
      const b = mk('button', 'btn small', r, 'Copy'); b.type = 'button'; b.setAttribute('aria-label', 'Copy the command: ' + label)
      b.addEventListener('click', () => copyText(c.textContent, c))
      cmds.push(c)
    }
    line('Read and change this board', 'write')
    line('Read this board only', 'read')
    who.addEventListener('change', () => { for (const c of cmds) c.textContent = App.grantCommand(c.dataset.level, id, who.value) })
    mk('div', 'note', p, 'Or in K2: Settings → Gardens → this Garden → Local Data.')
    const use = mk('div', 'note', p, 'Agents work on boards with k2 board, for example ')
    mk('code', '', use, 'k2 board list --board ' + id); use.appendChild(document.createTextNode('. For the rest, they read '))
    mk('code', '', use, 'k2 study board'); use.appendChild(document.createTextNode('.'))
    const now = mk('div', 'granted', p)
    App.store.chat.localAgents().then((rows) => {
      if (!who.isConnected) return
      for (const a of rows) { const o = mk('option', '', who, a.label === a.handle ? a.handle : a.label + ' (' + a.handle + ')'); o.value = a.handle }
    })
    App.store.boardAgents(id).then((r) => {
      if (!now.isConnected || !r.fromGrants) return
      mk('div', 'bgroup', now, 'Agents with access now')
      if (!r.agents.length) mk('div', 'empty', now, 'None yet.')
      for (const a of r.agents) {
        const row = mk('div', 'who', now); avatar(row, a)
        const n = mk('span', 'nm', row); mk('b', '', n, a.label)
        mk('span', 'lvl' + (a.level === 'write' ? ' edit' : ''), row, a.level === 'write' ? 'Read and change' : 'Read')
      }
    })
  }
  if (F.forAgents) btn(topright, 'people', 'For agents: let an agent work on this board', (el) => openPop(el, forAgentsPop, { cls: 'wide agents', side: 'down' }))

  // ---------- board chat: K2's own Agents and Conversation widgets, in the Garden's Chat column ----------
  // The board lists there the agents that can work on it (K2's grants on its data), and puts itself on the
  // message box as a chip, so each message says which board it's about. Unread counts are K2's own (the
  // Conversation column's tab).
  let chatGen = 0; let chatAgents = []; let chatBtn = null
  if (F.chat) {
    chatBtn = btn(topright, 'chat', 'Chat with this board’s agents', () => App.store.chat.toggle().then((ok) => { if (!ok) App.toast('The chat column isn’t on this page') }))
    chatBtn.classList.add('chatbtn')
    App.store.chat.watchColumn((open) => { chatBtn.classList.toggle('on', open === true); chatBtn.setAttribute('aria-expanded', String(open === true)) })
  }
  function steerChat() {
    if (!F.chat) return
    const id = App.store.boardId; const gen = ++chatGen
    chatAgents = []   // never the last board's agents while this one's load
    App.store.chat.context(boardName(), id)
    // A board may name the agent its chat opens with (a board file: its workspace's agent); it goes first.
    const first = App.store.chat.defaultAgent(id)
    if (first) App.store.chat.openAgent(first)
    App.store.chat.agents(id).then((agents) => {
      if (gen !== chatGen) return
      chatAgents = first ? agents.filter((a) => a.address === first).concat(agents.filter((a) => a.address !== first)) : agents
      App.store.chat.show(chatAgents.map((a) => a.address))
    })
  }
  App.on('board', steerChat)
  App.on('meta', () => { if (F.chat && App.store.boardId) App.store.chat.context(boardName(), App.store.boardId) })   // a renamed board
  topright.appendChild(themeBtn)   // For agents and Chat first, then light / dark
  App.on('toast', () => shareHits())

  sep(topright)
  btn(topright, 'history', 'Changes in the last day', (el) => openPop(el, historyPop, { cls: 'wide', side: 'down' }))
  const searchBtn = btn(topright, 'search', 'Search (⌘F)', (el) => openPop(el, searchPop, { cls: 'wide', side: 'down', onClose: clearFound }))
  btn(topright, 'frame', 'Frames', (el) => openPop(el, framesPop, { cls: 'wide', side: 'down' }))
  btn(topright, 'timer', 'Timer', (el) => openPop(el, timerPop, { side: 'down' }))
  btn(topright, 'vote', 'Voting', (el) => openPop(el, votingPop, { cls: 'wide', side: 'down' }))
  btn(topright, 'export', 'Export', (el) => openPop(el, exportPop, { cls: 'wide', side: 'down' }))
  sep(topright)
  btn(topright, 'play', 'Present frames', () => present(0), { cls: 'primary', label: 'Present' })
  App.on('search', () => openPop(searchBtn, searchPop, { cls: 'wide', side: 'down', keep: true, onClose: clearFound }))
  App.on('help', () => openPop(topright.querySelector('.btn.primary'), helpPop, { cls: 'wide', side: 'down' }))

  // ---------- zoom bar + minimap ----------
  const zoombar = $('zoombar'); const zoomBtns = {}
  zoomBtns.map = btn(zoombar, 'map', 'Minimap', () => { const m = $('minimap'); m.hidden = !m.hidden; zoomBtns.map.classList.toggle('on', !m.hidden); zoomBtns.map.setAttribute('aria-pressed', String(!m.hidden)); drawMinimap(); shareHits() })
  zoomBtns.snap = btn(zoombar, 'grid', 'Snap to grid (⇧G)', () => { App.snap = !App.snap; App.fire('tool') })
  sep(zoombar)
  btn(zoombar, 'minus', 'Zoom out (⌘−)', () => App.zoomCentre(0.8))
  const zoomLabel = mk('button', 'zoomlabel', zoombar, '100%'); zoomLabel.type = 'button'; zoomLabel.title = 'Zoom to 100%'
  zoomLabel.addEventListener('click', () => { const r = App.rect(); const s = 1; App.animateTo({ s, x: r.width / 2 - (r.width / 2 - App.cam.x) / App.cam.s, y: r.height / 2 - (r.height / 2 - App.cam.y) / App.cam.s }, 200) })
  btn(zoombar, 'plus', 'Zoom in (⌘+)', () => App.zoomCentre(1.25))
  btn(zoombar, 'fit', 'Fit everything (⇧1)', () => App.fitAll())
  btn(zoombar, 'more', 'Keyboard shortcuts (?)', (el) => openPop(el, helpPop, { cls: 'wide', side: 'up' }))

  const mm = $('minimap'); const mctx = mm.getContext('2d')
  let mmRaf = 0; let mmView = null
  function drawMinimap() {
    if (mm.hidden || mmRaf) return
    mmRaf = requestAnimationFrame(() => {
      mmRaf = 0
      const W = mm.width; const H = mm.height; const view = App.viewBox()
      const content = App.contentBox() || view
      const all = G.bounds([content, view]); const pad = 40 / Math.max(0.2, Math.min(1, App.cam.s))
      const box = { x: all.x - pad, y: all.y - pad, w: all.w + pad * 2, h: all.h + pad * 2 }
      const k = Math.min(W / box.w, H / box.h); const ox = (W - box.w * k) / 2 - box.x * k; const oy = (H - box.h * k) / 2 - box.y * k
      mmView = { k, ox, oy }
      const css = getComputedStyle(document.documentElement)
      mctx.clearRect(0, 0, W, H)
      const objs = model.all().sort((a, b) => (a.type === 'frame' ? -1 : 0) - (b.type === 'frame' ? -1 : 0))
      for (const o of objs) {
        const b = R.box(o.id); if (!b || o.type === 'connector' || o.type === 'comment') continue
        const x = b.x * k + ox; const y = b.y * k + oy; const w = Math.max(1.5, b.w * k); const h = Math.max(1.5, b.h * k)
        if (o.type === 'frame') { mctx.fillStyle = R.colors.fill(o.fill); mctx.fillRect(x, y, w, h); mctx.strokeStyle = css.getPropertyValue('--line').trim() || '#ccc'; mctx.lineWidth = 2; mctx.strokeRect(x, y, w, h); continue }
        mctx.fillStyle = o.type === 'sticky' ? R.colors.stickyColor(o.color) : o.type === 'shape' ? (o.fill === 'none' ? R.colors.ink(o.stroke) : R.colors.fill(o.fill)) : o.type === 'pen' ? R.colors.ink(o.stroke) : o.type === 'card' ? R.colors.ink(o.color) : o.type === 'mind' ? R.colors.ink(o.color) : css.getPropertyValue('--muted').trim() || '#999'
        mctx.globalAlpha = o.type === 'pen' || o.type === 'text' ? 0.6 : 1
        mctx.fillRect(x, y, w, h); mctx.globalAlpha = 1
      }
      const accent = css.getPropertyValue('--accent').trim() || '#2f6fed'
      mctx.strokeStyle = accent; mctx.lineWidth = 3
      mctx.fillStyle = accent; mctx.globalAlpha = 0.08
      mctx.fillRect(view.x * k + ox, view.y * k + oy, view.w * k, view.h * k); mctx.globalAlpha = 1
      mctx.strokeRect(view.x * k + ox, view.y * k + oy, view.w * k, view.h * k)
    })
  }
  function minimapGo(e) {
    if (!mmView) return
    const r = mm.getBoundingClientRect(); const sx = (e.clientX - r.left) * (mm.width / r.width); const sy = (e.clientY - r.top) * (mm.height / r.height)
    const wx = (sx - mmView.ox) / mmView.k; const wy = (sy - mmView.oy) / mmView.k
    const vr = App.rect(); App.cam.x = vr.width / 2 - wx * App.cam.s; App.cam.y = vr.height / 2 - wy * App.cam.s; App.applyCam()
  }
  mm.addEventListener('pointerdown', (e) => {
    e.stopPropagation(); minimapGo(e); try { mm.setPointerCapture(e.pointerId) } catch (_) {}
    const move = (m) => minimapGo(m); const up = () => { mm.removeEventListener('pointermove', move); mm.removeEventListener('pointerup', up) }
    mm.addEventListener('pointermove', move); mm.addEventListener('pointerup', up)
  })
  App.on('camera', () => { zoomLabel.textContent = Math.round(App.cam.s * 100) + '%'; drawMinimap(); placeCtx() })
  App.on('change', () => { drawMinimap(); undoBtn.disabled = !model.canUndo(); redoBtn.disabled = !model.canRedo() })

  // ---------- selection toolbar ----------
  const ctx = $('ctx')
  function buildCtx() {
    closePopIfCtx()
    ctx.textContent = ''
    const list = App.selected()
    if (App.editing) { editBar(); return }
    if (!list.length || App.presenting || App.tool !== 'select' || (list.length === 1 && list[0].type === 'comment')) { ctx.hidden = true; shareHits(); return }
    const one = list.length === 1 ? list[0] : null
    const types = new Set(list.map((o) => o.type)); const only = types.size === 1 ? [...types][0] : null
    const locked = list.every((o) => o.locked)
    if (locked) { btn(ctx, 'unlock', 'Unlock (⇧⌘L)', () => App.toggleLock(), { label: 'Unlock' }); finish(); return }
    const set = (f, filter) => App.setFields(f, filter)
    if (only === 'sticky' || only === 'text') textControls(list, one)
    else if (only !== 'shape' && list.some((o) => App.SIZED.includes(o.type))) sizeCtl(list.filter((o) => App.SIZED.includes(o.type)))
    if (only === 'shape') {
      const sb = btn(ctx, null, 'Shape', (el) => openPop(el, (p) => shapeGrid(p, one ? one.shape : '', (sh) => { App.lastShape = sh; set({ shape: sh }); closePop() })))
      sb.classList.add('shapebtn'); const NS = 'http://www.w3.org/2000/svg'; const svg = document.createElementNS(NS, 'svg'); svg.setAttribute('viewBox', '-2 -2 24 24'); svg.setAttribute('width', 20); svg.setAttribute('height', 20); const pth = document.createElementNS(NS, 'path'); pth.setAttribute('d', G.shapePath(list[0].shape, 20, 20)); svg.appendChild(pth); sb.appendChild(svg)
      swatchBtn(ctx, 'fill', list[0].fill, 'Fill', (n) => set({ fill: n }))
      swatchBtn(ctx, 'ink', list[0].stroke, 'Border', (n) => set({ stroke: n, strokeWidth: list[0].strokeWidth || 2 }))
      seg(ctx, [[0, 'None'], [1, 'Thin'], [2, 'Normal'], [4, 'Thick']], one ? one.strokeWidth : -1, (v) => set({ strokeWidth: v }), 'Border width')
      seg(ctx, [['solid', '—'], ['dashed', '- -'], ['dotted', '···']], one ? one.dash : '', (v) => set({ dash: v }), 'Border style')
      swatchBtn(ctx, 'ink', list[0].color, 'Text colour', (n) => set({ color: n }))
      sizeCtl(list)
    }
    if (only === 'connector') {
      seg(ctx, [['straight', 'Straight'], ['elbow', 'Elbow'], ['curved', 'Curved']], one ? one.route : '', (v) => { set({ route: v }); App.lastConnector = Object.assign({}, App.lastConnector, { route: v }) }, 'Line type')
      swatchBtn(ctx, 'ink', list[0].stroke, 'Colour', (n) => { set({ stroke: n }); App.lastConnector = Object.assign({}, App.lastConnector, { stroke: n }) })
      seg(ctx, [[1, '1'], [2, '2'], [4, '4'], [6, '6']], one ? one.strokeWidth : -1, (v) => set({ strokeWidth: v }), 'Thickness')
      seg(ctx, [['solid', '—'], ['dashed', '- -'], ['dotted', '···']], one ? one.dash : '', (v) => set({ dash: v }), 'Style')
      const capLabel = { none: 'None', arrow: 'Arrow', triangle: 'Triangle', circle: 'Circle', diamond: 'Diamond', bar: 'Bar' }
      btn(ctx, null, 'Line ends', (el) => openPop(el, (p) => {
        mk('div', 'pop-h', p, 'Start'); seg(p, S.CAPS.map((c) => [c, capLabel[c]]), list[0].startCap, (v) => set({ startCap: v }))
        mk('div', 'pop-h', p, 'End'); seg(p, S.CAPS.map((c) => [c, capLabel[c]]), list[0].endCap, (v) => { set({ endCap: v }); App.lastConnector = Object.assign({}, App.lastConnector, { endCap: v }) })
      }), { label: 'Ends' })
      if (one) btn(ctx, 'text', 'Label', () => App.startEdit(one.id, 'label'))
    }
    if (only === 'pen') {
      swatchBtn(ctx, 'ink', list[0].stroke, 'Colour', (n) => set({ stroke: n }))
      seg(ctx, [[2, 'Fine'], [3, 'Medium'], [6, 'Bold'], [12, 'Marker']], one ? one.strokeWidth : -1, (v) => set({ strokeWidth: v }), 'Thickness')
    }
    if (only === 'frame') {
      swatchBtn(ctx, 'fill', list[0].fill, 'Background', (n) => set({ fill: n }))
      if (one) { btn(ctx, 'text', 'Rename', () => App.startEdit(one.id, 'title'), { label: 'Rename' }); btn(ctx, 'play', 'Present from this frame', () => present(frames().findIndex((f) => f.id === one.id))) }
    }
    if (only === 'card') {
      swatchBtn(ctx, 'ink', list[0].color, 'Colour', (n) => set({ color: n }))
      if (one) btn(ctx, 'card', 'Fields', (el) => openPop(el, (p) => cardPop(p, one), { avoid: selRect() }), { label: 'Fields' })
    }
    if (only === 'table' && one) {
      swatchBtn(ctx, 'fill', one.fill, 'Background', (n) => set({ fill: n }))
      // Rows and columns keep their ids (rowIds, colIds, and colW when set) in step with every change.
      const cols = () => Math.max(0, ...one.rows.map((r) => r.length))
      const colW = (f) => (Array.isArray(one.colW) && one.colW.length ? { colW: f(one.colW.slice()) } : {})
      btn(ctx, 'row', 'Add row', () => set({ rows: one.rows.concat([Array.from({ length: cols() }, () => '')]), rowIds: one.rowIds.concat(S.itemId('rw')) }))
      btn(ctx, 'col', 'Add column', () => { const n = cols(); set(Object.assign({ rows: one.rows.map((r) => r.concat(Array.from({ length: n + 1 - r.length }, () => ''))), colIds: one.colIds.concat(S.itemId('cl')), w: one.w + 160 }, colW((w) => w.concat(0)))) })
      btn(ctx, 'minus', 'Remove last row', () => { if (one.rows.length > 1) set({ rows: one.rows.slice(0, -1), rowIds: one.rowIds.slice(0, -1) }) })
      btn(ctx, 'close', 'Remove last column', () => { const n = cols(); if (n > 1) set(Object.assign({ rows: one.rows.map((r) => r.slice(0, n - 1)), colIds: one.colIds.slice(0, n - 1), w: Math.max(120, one.w - 160) }, colW((w) => w.slice(0, n - 1)))) })
    }
    if (only === 'mind') {
      swatchBtn(ctx, 'ink', list[0].color, 'Branch colour', (n) => set({ color: n }))
      if (one) { btn(ctx, 'child', 'Add child (Tab)', () => mindChild(one.id)); if (one.parent) btn(ctx, 'plus', 'Add sibling (Enter while editing)', () => mindSibling(one.id)) }
    }
    if (only === 'comment' && one) btn(ctx, 'comment', 'Open', () => openComment(one.id), { label: 'Open' })
    if (list.length > 1) {
      if (ctx.children.length) sep(ctx)
      btn(ctx, 'alignL', 'Align left', () => App.align('left')); btn(ctx, 'alignC', 'Align centres', () => App.align('center')); btn(ctx, 'alignR', 'Align right', () => App.align('right'))
      btn(ctx, 'alignT', 'Align top', () => App.align('top')); btn(ctx, 'alignM', 'Align middles', () => App.align('middle')); btn(ctx, 'alignB', 'Align bottom', () => App.align('bottom'))
      if (list.length > 2) { btn(ctx, 'distH', 'Distribute horizontally', () => App.distribute('h')); btn(ctx, 'distV', 'Distribute vertically', () => App.distribute('v')) }
      sep(ctx)
      if (list.some((o) => o.group)) btn(ctx, 'ungroup', 'Ungroup (⇧⌘G)', () => App.ungroup())
      else btn(ctx, 'group', 'Group (⌘G)', () => App.group())
    }
    if (ctx.children.length) sep(ctx)
    if (list.some((o) => ['sticky', 'card', 'shape', 'text', 'mind', 'image'].includes(o.type))) {
      btn(ctx, 'smile', 'React', (el) => openPop(el, (p) => {
        const row = mk('div', 'emojis', p)
        const pick = (em) => { for (const o of App.selected()) App.toggleReaction(o.id, em); closePop() }
        for (const em of S.REACTIONS.filter((x) => !S.STAMPS.includes(x))) { const b = mk('button', 'emo', row, em); b.type = 'button'; b.addEventListener('click', () => pick(em)) }
        mk('div', 'pop-h', p, 'Stamps')
        const srow = mk('div', 'emojis', p)
        for (const em of S.STAMPS) { const b = mk('button', 'emo big', srow, em); b.type = 'button'; b.title = 'Stamp ' + em; b.addEventListener('click', () => pick(em)) }
      }))
    }
    if (list.some((o) => ['sticky', 'card'].includes(o.type))) btn(ctx, 'tag', 'Tags', (el) => openPop(el, (p) => tagsPop(p)))
    if (list.length === 1 && one.type !== 'comment') btn(ctx, 'comment', 'Comment', () => newComment([one.x + (one.w || 0) - 14, one.y - 14], one.id))
    if (one && (one.type === 'card' || one.type === 'text')) btn(ctx, 'checklist', 'Checklist', (el) => openPop(el, (p) => checksPop(p, one.id), { avoid: selRect() }))
    if (one && one.type === 'image' && one.src) btn(ctx, 'crop', 'Crop', (el) => openPop(el, (p) => cropPop(p, one.id), { cls: 'wide', avoid: selRect() }))
    if (one && !['connector', 'comment', 'pen'].includes(one.type)) btn(ctx, 'link', one.link ? 'Link: ' + one.link : 'Add a link', (el) => openPop(el, (p) => linkPop(p, one.id, true), { avoid: selRect() }))
    btn(ctx, 'lock', 'Lock (⇧⌘L)', () => App.toggleLock())
    btn(ctx, 'more', 'More', (el) => openPop(el, (p) => {
      const item = (ic, label, f, key) => { const b = mk('button', 'menuitem', p); b.type = 'button'; b.setAttribute('role', 'menuitem'); b.appendChild(icon(ic, 16)); mk('span', '', b, label); if (key) mk('kbd', '', b, key); b.addEventListener('click', () => { closePop(); f() }) }
      item('front', 'Bring to front', () => App.layer('front'), '⌘]'); item('up', 'Bring forward', () => App.layer('forward'), ']')
      item('down', 'Send backward', () => App.layer('backward'), '['); item('back', 'Send to back', () => App.layer('back'), '⌘[')
      mk('div', 'menusep', p)
      item('copy', 'Duplicate', () => App.duplicate(), '⌘D'); item('copy', 'Copy', () => App.copyCmd(false), '⌘C')
      item('fit', 'Zoom to selection', () => App.fitTo(App.selBox(), { max: 2 }), '⇧2')
      if (App.canTidy()) item('grid', 'Tidy up', () => App.tidy())
      if (App.styleSource()) item('brush', 'Copy style', () => App.copyStyle(), '⌥⌘C')
      if (App.hasStyle()) item('brush', 'Paste style', () => App.pasteStyle(), '⌥⌘V')
      mk('div', 'menusep', p)
      item('trash', 'Delete', () => App.del(), '⌫')
    }, { menu: true }))
    finish()
    function finish() { ctx.hidden = false; placeCtx(); shareHits() }
  }
  // Text controls for stickies, text and shapes. The same ones show while typing.
  function textControls(list, one) {
    const set = (f) => App.setFields(f); const only = list[0].type
    if (only === 'sticky') swatchBtn(ctx, 'sticky', list[0].color, 'Colour', (n) => { App.lastSticky = n; set({ color: n }); updateToolDots() })
    else swatchBtn(ctx, 'ink', list[0].color, 'Text colour', (n) => set({ color: n }))
    sizeCtl(list)
    if (only === 'text') {
      seg(ctx, [[true, 'Bold', 'bold']], list.every((o) => o.bold), () => set({ bold: !list.every((o) => o.bold) }))
      seg(ctx, [[true, 'Italic', 'italic']], list.every((o) => o.italic), () => set({ italic: !list.every((o) => o.italic) }))
    }
    if (only !== 'shape') seg(ctx, [['left', 'Left', 'alignL'], ['center', 'Centre', 'alignC'], ['right', 'Right', 'alignR']], one ? one.align || (only === 'sticky' ? 'center' : 'left') : '', (v) => set({ align: v }))
  }
  // Text size: smaller · current size (opens the list) · bigger. Mixed types step together.
  const SIZE_PRESETS = [12, 14, 18, 24, 32, 48, 64, 96]
  function sizeCtl(list) {
    if (!list.length) return
    const vals = new Set(list.map((o) => o.fontSize || 0)); const v = vals.size === 1 ? [...vals][0] : null
    const wrap = mk('div', 'seg tsize', ctx); wrap.title = 'Text size'
    const part = (cls, text, title, onClick) => {
      const b = mk('button', 'segb ' + cls, wrap, text); b.type = 'button'; b.title = title; b.setAttribute('aria-label', title)
      b.addEventListener('pointerdown', (e) => e.stopPropagation())
      b.addEventListener('click', (e) => { e.stopPropagation(); onClick(b) })
      return b
    }
    part('a-sm', 'A', 'Smaller text (⇧⌘,)', () => App.textSize(-1))
    part('tval', v === null ? 'Mixed' : v === 0 ? 'Auto' : String(v), 'Text size', (b) => openPop(b, (p) => {
      mk('div', 'pop-h', p, 'Text size')
      const opts = (list.every((o) => o.type === 'sticky') ? [0] : []).concat(SIZE_PRESETS)
      for (const n of opts) {
        const it = mk('button', 'menuitem' + (n === v ? ' on' : ''), p); it.type = 'button'
        mk('span', '', it, n ? String(n) : 'Auto (fit the note)')
        it.addEventListener('click', () => { closePop(); App.setFields({ fontSize: n }, (o) => App.SIZED.includes(o.type)) })
      }
    }, { cls: 'sizes' }))
    part('a-lg', 'A', 'Bigger text (⇧⌘.)', () => App.textSize(1))
  }
  // While typing: the text controls stay up, so size and style are where the eye is.
  function editBar() {
    const ed = App.editing; const o = model.get(ed.id)
    if (!o || ed.field !== 'text' || !App.SIZED.includes(o.type) || App.presenting) { ctx.hidden = true; shareHits(); return }
    if (o.type === 'shape') { swatchBtn(ctx, 'ink', o.color, 'Text colour', (n) => App.setFields({ color: n })); sizeCtl([o]) } else textControls([o], o)
    ctx.hidden = false; placeCtx(); shareHits()
  }
  // Clicking the bar or its popovers mustn't take focus from the text being typed.
  for (const el of [ctx, pop]) el.addEventListener('mousedown', (e) => { if (App.editing && !/INPUT|TEXTAREA/.test(e.target.tagName)) e.preventDefault() })
  function closePopIfCtx() { if (popAnchor instanceof Node && ctx.contains(popAnchor)) closePop() }
  function placeCtx() {
    if (ctx.hidden) return
    const b = App.editing ? R.box(App.editing.id) : App.selBox(); if (!b) return
    const [x1, y1] = App.toScreen(b.x, b.y); const [x2, y2] = App.toScreen(b.x + b.w, b.y + b.h)
    const w = ctx.offsetWidth; const h = ctx.offsetHeight
    const one = !App.editing && App.selected().length === 1 ? App.selected()[0] : null
    const lift = one && ['sticky', 'text', 'shape', 'image', 'card', 'pen'].includes(one.type) && !one.locked ? 50 : 22
    let x = (x1 + x2) / 2 - w / 2
    x = Math.max(76, Math.min(innerWidth - w - 12, x))
    // Above by default; below when above is off screen or would cover other content (a heading over a sticky).
    const above = y1 - h - lift; const below = y2 + 22
    const fits = (y) => y >= 64 && y + h <= innerHeight - 12
    let y
    if (!fits(above)) y = fits(below) ? below : Math.max(64, y1 + 12)
    else if (!fits(below)) y = above
    else { const ca = covers(x, above, w, h); y = ca && covers(x, below, w, h) < ca ? below : above }
    ctx.style.left = Math.round(x) + 'px'; ctx.style.top = Math.round(y) + 'px'
  }
  // Screen area of other objects the bar would hide at (x, y). Frames and lines don't count: the bar may sit over them.
  function covers(x, y, w, h) {
    const mine = App.editing ? new Set([App.editing.id]) : App.sel; let area = 0
    for (const o of model.all()) {
      if (mine.has(o.id) || o.type === 'frame' || o.type === 'connector' || o.type === 'pen') continue
      const b = R.box(o.id); if (!b) continue
      const [ax, ay] = App.toScreen(b.x, b.y); const [bx, by] = App.toScreen(b.x + b.w, b.y + b.h)
      const ox = Math.min(x + w, bx) - Math.max(x, ax); const oy = Math.min(y + h, by) - Math.max(y, ay)
      if (ox > 0 && oy > 0) area += ox * oy
    }
    return area
  }
  let ctxRaf = 0
  const refreshCtx = () => { if (!ctxRaf) ctxRaf = requestAnimationFrame(() => { ctxRaf = 0; buildCtx() }) }
  App.on('selection', refreshCtx)
  // Screen readers hear what's selected (polite, after the selection settles).
  const TYPE_WORD = { sticky: 'Sticky note', text: 'Text', shape: 'Shape', connector: 'Line', pen: 'Drawing', frame: 'Frame', card: 'Card', comment: 'Comment', mind: 'Mind map node', table: 'Table', image: 'Image', group: 'Group' }
  let saidT = 0
  App.announce = (msg) => { const r = $('announce'); if (!r) return; r.textContent = ''; setTimeout(() => { r.textContent = msg }, 30) }
  App.on('selection', () => {
    clearTimeout(saidT)
    saidT = setTimeout(() => {
      const list = App.selected(); if (!list.length) return
      if (list.length > 1) { App.announce(list.length + ' items selected'); return }
      const o = list[0]; const t = String(o.title || o.text || o.name || '').replace(/\s+/g, ' ').trim().slice(0, 80)
      App.announce((TYPE_WORD[o.type] || 'Item') + (t ? ' “' + t + '”' : '') + ' selected' + (o.locked ? ', locked' : ''))
    }, 250)
  })
  App.on('tool', refreshCtx)
  App.on('change', (ids) => { if ([...ids].some((id) => App.sel.has(id) || (App.editing && App.editing.id === id))) { if (popAnchor instanceof Node && ctx.contains(popAnchor)) placeCtx(); else refreshCtx() } })
  App.on('overlay', placeCtx)

  function tagsPop(p) {
    mk('div', 'pop-h', p, 'Tags')
    const objs = App.selected().filter((o) => ['sticky', 'card'].includes(o.type))
    const chips = mk('div', 'chips', p)
    const all = new Set(); for (const o of objs) for (const t of o.tags || []) all.add(t)
    const draw = () => {
      chips.textContent = ''
      for (const t of all) { const c = mk('span', 'chip', chips, t); const x = mk('button', 'x', c); x.type = 'button'; x.appendChild(icon('close', 12)); x.addEventListener('click', () => { all.delete(t); apply(); draw() }) }
    }
    const apply = () => model.apply(objs.map((o) => ({ op: 'set', id: o.id, f: { tags: [...all] } })))
    const input = mk('input', 'field', p); input.placeholder = 'Add a tag, press Enter'
    input.addEventListener('keydown', (e) => { if (e.key === 'Enter' && input.value.trim()) { all.add(input.value.trim()); input.value = ''; apply(); draw() } })
    input.dataset.autofocus = '1'; draw()
  }
  function cardPop(p, o) {
    mk('div', 'pop-h', p, 'Card fields')
    const field = (label, key, type, opts) => {
      const row = mk('label', 'frow', p); mk('span', '', row, label)
      let input
      if (opts) { input = mk('select', 'field', row); for (const v of opts) { const op = mk('option', '', input, v || '—'); op.value = v } }
      else { input = mk('input', 'field', row); input.type = type || 'text' }
      input.value = o[key] || ''
      input.addEventListener('change', () => model.apply([{ op: 'set', id: o.id, f: { [key]: input.value } }]))
    }
    field('Assignee', 'assignee'); field('Due', 'due', 'date'); field('Status', 'status', null, ['', 'To do', 'In progress', 'Blocked', 'In review', 'Done'])
  }

  // ---------- mind map ----------
  function mindChild(id) {
    const p = model.get(id); if (!p) return
    const kids = [...R.childrenOf(id)].map((k) => model.get(k)).filter(Boolean)
    const root = (() => { let r = p; while (r.parent && model.get(r.parent)) r = model.get(r.parent); return r })()
    const left = p.parent ? p.x < root.x : kids.filter((k) => k.x < p.x).length < kids.filter((k) => k.x >= p.x).length
    const x = left ? p.x - 240 : p.x + p.w + 80
    const y = kids.length ? Math.max(...kids.map((k) => k.y + k.h)) + 20 : p.y + p.h / 2 - 24
    const nid = App.create('mind', { x: Math.round(x), y: Math.round(y), w: 160, h: 48, text: '', parent: id, color: p.parent ? p.color : ['blue', 'green', 'orange', 'pink', 'teal', 'violet'][kids.length % 6] })
    App.startEdit(nid)
  }
  function mindSibling(id) { const o = model.get(id); if (o && o.parent) mindChild(o.parent); else if (o) mindChild(id) }
  App.on('mind-child', mindChild); App.on('mind-sibling', mindSibling)

  // ---------- comments ----------
  function newComment(at, target) {
    const id = App.create('comment', { x: Math.round(at[0]), y: Math.round(at[1]), text: '', target: target || '' })
    if (id) openComment(id, true)
  }
  App.on('comment-new', (p, target) => newComment(p, target))
  App.on('comment-open', (id) => openComment(id))
  function when(iso) { if (!iso) return ''; const d = new Date(iso); const m = Math.round((Date.now() - d) / 60000); return m < 1 ? 'just now' : m < 60 ? m + ' min ago' : m < 1440 ? Math.round(m / 60) + ' h ago' : d.toLocaleDateString() }
  function openComment(id, fresh) {
    const el = R.el(id); if (!el) return
    const anchor = el.querySelector('.pin') || el
    openPop(anchor, (p) => {
      const o = model.get(id)
      const head = mk('div', 'thead', p); mk('span', 'pop-h', head, o.resolved ? 'Resolved' : 'Comment')
      if (!fresh) {
        btn(head, o.resolved ? 'reset' : 'check', o.resolved ? 'Reopen' : 'Resolve', () => { model.apply([{ op: 'set', id, f: { resolved: !o.resolved } }]); closePop() })
        btn(head, 'trash', 'Delete comment', () => { model.apply([{ op: 'del', id }]); closePop() })
      }
      const msgs = mk('div', 'thread', p)
      const msg = (by, at, text) => { const name = model.nameOf(by); const m = mk('div', 'msg', msgs); const h = mk('div', 'mh', m); mk('span', 'av', h, (name || '?').slice(0, 1).toUpperCase()); mk('b', '', h, name || 'Someone'); mk('span', 'when', h, when(at)); mk('div', 'mt', m, text) }
      if (o.text) msg(o.createdBy, o.createdAt, o.text)
      for (const r of o.replies || []) msg(r.by, r.at, r.text)
      const ta = mk('textarea', 'field', p); ta.rows = 3; ta.placeholder = o.text ? 'Reply' : 'Write a comment'
      const row = mk('div', 'frow end', p)
      const send = () => {
        const t = ta.value.trim(); if (!t) return
        if (!o.text) { if (!model.apply([{ op: 'set', id, f: { text: t } }])) return }
        else if (!model.apply([{ op: 'set', id, f: { replies: (o.replies || []).concat([{ id: S.itemId('rp'), by: model.me, at: new Date().toISOString(), text: t }]) } }])) return   // refused (too long): the text stays to edit
        closePop()
      }
      btn(row, null, 'Send (⌘Enter)', send, { label: o.text ? 'Reply' : 'Comment', cls: 'primary' })
      ta.addEventListener('keydown', (e) => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) send() })
      ta.dataset.autofocus = '1'
    }, { cls: 'wide', side: 'right', onClose: () => { const o = model.get(id); if (o && !o.text) model.apply([{ op: 'del', id }], { noUndo: true }) } })
  }

  // ---------- templates ----------
  function templatesPop(p) {
    mk('div', 'pop-h', p, 'Templates')
    const grid = mk('div', 'tgrid', p)
    const desc = { kickoff: 'Goals, roles, milestones, risks and first steps', kanban: 'Columns and cards for tracking work', retro: 'What went well, what to improve, actions', flowchart: 'Start, steps, a decision and an end', storymap: 'Activities, steps and stories by release', brainstorm: 'A big question and a ring of notes', mindmap: 'A central idea with four branches' }
    const ic = { kickoff: 'flag', kanban: 'card', retro: 'sticky', flowchart: 'shape', storymap: 'table', brainstorm: 'smile', mindmap: 'mind' }
    for (const [k, name] of Object.entries(S.TEMPLATE_NAMES)) {
      const b = mk('button', 'tcard', grid); b.type = 'button'
      const th = mk('div', 'tthumb t-' + k, b); th.appendChild(icon(ic[k], 26))
      mk('b', '', b, name); mk('span', '', b, desc[k])
      b.addEventListener('click', () => { closePop(); insertTemplate(k) })
    }
  }
  function insertTemplate(k) {
    const ops = S.templates((t) => App.mintId(t))[k](0, 0)
    const boxes = ops.map((op) => ({ x: op.obj.x || 0, y: op.obj.y || 0, w: op.obj.w || (S.DEFAULTS[op.obj.type] || {}).w || 0, h: op.obj.h || (S.DEFAULTS[op.obj.type] || {}).h || 0 }))
    const b = G.bounds(boxes)
    const content = App.contentBox()
    const at = content ? [content.x + content.w + 160, content.y] : App.freeSpot(App.viewCentre(), b.w, b.h)
    for (const op of ops) if (op.obj.type !== 'connector') { op.obj.x = Math.round((op.obj.x || 0) + at[0]); op.obj.y = Math.round((op.obj.y || 0) + at[1]) }
    model.apply(ops)
    App.select([])
    App.fitTo({ x: at[0], y: at[1], w: b.w, h: b.h }, { max: 1 })
    App.toast(S.TEMPLATE_NAMES[k] + ' added')
  }

  // ---------- frames + presentation ----------
  const frames = () => model.all().filter((o) => o.type === 'frame').sort((a, b) => (a.order || 0) - (b.order || 0) || a.y - b.y || a.x - b.x)
  function framesPop(p) {
    mk('div', 'pop-h', p, 'Frames')
    const list = frames()
    if (!list.length) { mk('div', 'empty', p, 'No frames yet. Press F and drag to draw one.'); return }
    const ul = mk('div', 'flist', p)
    list.forEach((f, i) => {
      const row = mk('div', 'frow2', ul)
      mk('span', 'num', row, String(i + 1))
      const name = mk('button', 'fname', row, f.title || 'Frame'); name.type = 'button'
      name.addEventListener('click', () => { App.select([f.id]); App.fitTo(R.box(f.id), { max: 2, pad: 60 }) })
      const move = (d) => {
        const l = frames(); const j = i + d; if (j < 0 || j >= l.length) return
        ;[l[i], l[j]] = [l[j], l[i]]
        model.apply(l.map((x, n) => ({ op: 'set', id: x.id, f: { order: n } })))
        p.textContent = ''; framesPop(p)
      }
      btn(row, 'up', 'Move up', () => move(-1), { size: 16 }); btn(row, 'down', 'Move down', () => move(1), { size: 16 })
    })
    const row = mk('div', 'frow end', p)
    btn(row, 'play', 'Present', () => { closePop(); present(0) }, { label: 'Present', cls: 'primary' })
  }
  let presIdx = 0
  const presBar = $('present')
  function present(i) {
    const list = frames(); if (!list.length) { App.toast('Add a frame (F) to present it'); return }
    closePop(); App.select([]); App.endEdit(true)
    App.presenting = true; document.getElementById('app').classList.add('presenting')
    presIdx = Math.max(0, Math.min(list.length - 1, i || 0))
    showSlide()
  }
  function showSlide() {
    const list = frames(); const f = list[presIdx]; if (!f) return exitPresent()
    App.fitTo(R.box(f.id), { pad: 28, top: 52, max: 4 })
    presBar.textContent = ''; presBar.hidden = false
    btn(presBar, 'left', 'Previous (←)', () => go(-1))
    mk('span', 'ptitle', presBar, (presIdx + 1) + ' / ' + list.length + '  ·  ' + (f.title || 'Frame'))
    btn(presBar, 'right', 'Next (→)', () => go(1))
    sep(presBar)
    btn(presBar, 'close', 'Exit (Esc)', exitPresent, { label: 'Exit' })
    shareHits()
  }
  function go(d) { const n = frames().length; presIdx = Math.max(0, Math.min(n - 1, presIdx + d)); showSlide() }
  function exitPresent() { App.presenting = false; document.getElementById('app').classList.remove('presenting'); presBar.hidden = true; shareHits() }
  App.on('present-key', (e) => {
    if (e.key === 'Escape') exitPresent()
    else if (['ArrowRight', 'ArrowDown', 'PageDown', ' ', 'Enter'].includes(e.key)) { e.preventDefault(); go(1) }
    else if (['ArrowLeft', 'ArrowUp', 'PageUp'].includes(e.key)) { e.preventDefault(); go(-1) }
  })
  window.addEventListener('resize', () => { if (App.presenting) showSlide() })

  // ---------- search ----------
  let found = []
  function searchPop(p) {
    const input = mk('input', 'field search', p); input.placeholder = 'Search this board'
    const info = mk('div', 'sinfo', p); const results = mk('div', 'slist', p)
    let idx = -1
    const arr = (v) => (Array.isArray(v) ? v : [])   // never trust a stored value's shape
    const textOf = (o) => [o.text, o.title, o.label, o.assignee, o.status, arr(o.tags).join(' '), arr(o.rows).map((r) => arr(r).join(' ')).join(' '), arr(o.replies).map((r) => (r && r.text) || '').join(' ')].filter((x) => typeof x === 'string' && x).join(' ')
    const run = () => {
      for (const id of found) { const el = R.el(id); if (el) el.classList.remove('found') }
      const q = input.value.trim().toLowerCase(); results.textContent = ''; found = []; idx = -1
      if (!q) { info.textContent = ''; return }
      const hits = model.all().filter((o) => textOf(o).toLowerCase().includes(q))
      found = hits.map((o) => o.id)
      info.textContent = hits.length ? hits.length + ' found' : 'Nothing found'
      for (const o of hits.slice(0, 50)) {
        const b = mk('button', 'sres', results); b.type = 'button'
        b.appendChild(icon({ sticky: 'sticky', text: 'text', shape: 'shape', connector: 'line', frame: 'frame', card: 'card', table: 'table', mind: 'mind', comment: 'comment' }[o.type] || 'more', 16))
        const t = textOf(o); const i = t.toLowerCase().indexOf(q); const start = Math.max(0, i - 24)
        mk('span', '', b, (start ? '…' : '') + t.slice(start, start + 70).replace(/\s+/g, ' '))
        b.addEventListener('click', () => focusHit(found.indexOf(o.id)))
        const el = R.el(o.id); if (el) el.classList.add('found')
      }
    }
    const focusHit = (i) => {
      if (!found.length) return
      idx = (i + found.length) % found.length
      results.querySelectorAll('.sres').forEach((b, j) => b.classList.toggle('on', j === idx))
      App.select([found[idx]], { raw: true }); App.fitTo(R.box(found[idx]), { max: 1.5, pad: 200 })
    }
    input.addEventListener('input', run)
    input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); focusHit(idx + (e.shiftKey ? -1 : 1)) } })
    input.dataset.autofocus = '1'
  }
  // Closing the search clears its highlights (the panel hides at once, so no transition end to wait for).
  function clearFound() { for (const id of found) { const el = R.el(id); if (el) el.classList.remove('found') } found = [] }

  // ---------- timer ----------
  const hud = $('hud')
  function timerPop(p) {
    mk('div', 'pop-h', p, 'Timer')
    const row = mk('div', 'presets', p)
    for (const m of [1, 3, 5, 10, 15, 25]) { const b = mk('button', 'preset', row, m + ' min'); b.type = 'button'; b.addEventListener('click', () => { startTimer(m * 60); closePop() }) }
  }
  function startTimer(sec) { model.apply([{ op: 'meta', f: { timer: { state: 'running', duration: sec, endsAt: Date.now() + sec * 1000, remaining: sec } } }]) }
  function timerLeft(t) { return t.state === 'running' ? Math.max(0, Math.round((t.endsAt - Date.now()) / 1000)) : (t.remaining || 0) }
  const fmt = (s) => Math.floor(s / 60) + ':' + String(s % 60).padStart(2, '0')
  let hudKey = ''
  function drawHud() {
    const t = model.meta.timer; const v = model.meta.voting
    const timerOn = t && t.state && t.state !== 'off'
    const key = JSON.stringify([t, v, timerOn && timerLeft(t), votesLeft(), activity && [activity.label, when(activity.at)]])
    if (key === hudKey) return; hudKey = key
    hud.textContent = ''
    if (timerOn) {
      const left = timerLeft(t)
      if (t.state === 'running' && left === 0) { model.apply([{ op: 'meta', f: { timer: { state: 'done', duration: t.duration, remaining: 0 } } }], { noUndo: true }); return }
      const pill = mk('div', 'pill timer' + (t.state === 'done' ? ' done' : '') + (left <= 10 && t.state === 'running' ? ' soon' : ''), hud)
      pill.appendChild(icon('timer', 16)); mk('span', 'time', pill, t.state === 'done' ? 'Time’s up' : fmt(left))
      if (t.state === 'running') btn(pill, 'pause', 'Pause', () => model.apply([{ op: 'meta', f: { timer: { state: 'paused', duration: t.duration, remaining: left } } }], { noUndo: true }), { size: 16 })
      else if (t.state === 'paused') btn(pill, 'play', 'Resume', () => model.apply([{ op: 'meta', f: { timer: { state: 'running', duration: t.duration, endsAt: Date.now() + left * 1000, remaining: left } } }], { noUndo: true }), { size: 16 })
      if (t.state !== 'done') btn(pill, 'plus', 'Add a minute', () => model.apply([{ op: 'meta', f: { timer: Object.assign({}, t, { endsAt: (t.endsAt || 0) + 60000, remaining: (t.remaining || 0) + 60 }) } }], { noUndo: true }), { size: 16 })
      btn(pill, 'close', 'Stop', () => model.apply([{ op: 'meta', f: { timer: { state: 'off' } } }], { noUndo: true }), { size: 16 })
    }
    if (v && v.active) {
      const pill = mk('div', 'pill voting', hud)
      pill.appendChild(icon('vote', 16)); mk('span', '', pill, 'Voting · ' + votesLeft() + ' of ' + (v.perPerson || 5) + ' votes left')
      btn(pill, null, 'End voting', endVoting, { label: 'End', cls: 'primary small' })
    }
    if (activity) {
      const a = activity
      const pill = mk('div', 'pill activity', hud); pill.setAttribute('role', 'status')
      mk('span', 'who', pill, a.names[0].slice(0, 1).toUpperCase())
      mk('span', 'what', pill, a.label); mk('span', 'ago', pill, when(a.at))
      btn(pill, null, 'Show what changed', () => showActivity(a), { label: 'Show', cls: 'small' })
      btn(pill, 'close', 'Dismiss', () => endActivity(), { size: 16 })
    }
    shareHits()
  }
  setInterval(drawHud, 250)
  App.on('change', () => { if (hint && model.all().length) { hint.remove(); hint = null } })
  App.on('meta', () => { drawHud(); R.setVoting(!!(model.meta.voting && model.meta.voting.active)); drawName() })

  // ---------- agent activity ----------
  // A Garden has no live presence. Each object records who last changed it
  // (updatedBy/updatedAt), so after a load show what agents changed lately.
  let activity = null; let activityTimer = 0
  function noteActivity() {
    const cutoff = Date.now() - 10 * 60000; const by = new Map()
    for (const o of model.all()) {
      const who = o.updatedBy; const t = Date.parse(o.updatedAt)
      if (!who || model.isMe(who) || !(t > cutoff)) continue   // the person's own edits, under any key or older name
      const e = by.get(who) || { ids: [], at: 0 }; e.ids.push(o.id); e.at = Math.max(e.at, t); by.set(who, e)
    }
    if (!by.size) return
    const keys = [...by.keys()].sort((a, b) => by.get(b).at - by.get(a).at)
    const ids = keys.flatMap((n) => by.get(n).ids); const at = Math.max(...keys.map((n) => by.get(n).at))
    const names = keys.map((k) => model.nameOf(k))
    const who = names.length === 1 ? names[0] : names.length === 2 ? names[0] + ' and ' + names[1] : names[0] + ' and ' + (names.length - 1) + ' others'
    activity = { names, ids, at: new Date(at).toISOString(), label: who + ' changed ' + ids.length + (ids.length === 1 ? ' item' : ' items') }
    drawHud()
    activityTimer = setTimeout(endActivity, 20000)
  }
  function endActivity() { clearTimeout(activityTimer); activity = null; drawHud() }
  function showActivity(a) {
    const ids = a.ids.filter((id) => model.get(id)); endActivity(); if (!ids.length) return
    App.select(ids, { raw: true }); App.fitTo(App.selBox(), { max: 1.5, pad: 160 })
    for (const id of ids) { const el = R.el(id); if (el) { el.classList.add('found'); setTimeout(() => el.classList.remove('found'), 3200) } }
  }

  // ---------- voting ----------
  function votesLeft() {
    const v = model.meta.voting; if (!v || !v.active) return 0
    let used = 0; for (const o of model.all()) for (const [k, n] of Object.entries(o.votes || {})) if (model.isMe(k)) used += n
    return Math.max(0, (v.perPerson || 5) - used)
  }
  App.on('vote', (id, d) => {
    const o = model.get(id); if (!o) return
    if (d > 0 && votesLeft() <= 0) { App.toast('No votes left'); return }
    const votes = Object.assign({}, o.votes || {})
    if (d > 0) votes[model.me] = (votes[model.me] || 0) + 1
    else { const k = votes[model.me] ? model.me : Object.keys(votes).find((x) => model.isMe(x)); if (!k) return; votes[k] -= 1; if (!votes[k]) delete votes[k] }
    model.apply([{ op: 'set', id, f: { votes } }])
  })
  function votingPop(p) {
    const v = model.meta.voting || {}
    mk('div', 'pop-h', p, 'Voting')
    if (v.active) {
      mk('div', 'note', p, 'Click the badge on a note to vote. Shift-click takes a vote back.')
      const row = mk('div', 'frow end', p); btn(row, null, 'End voting', () => { closePop(); endVoting() }, { label: 'End voting', cls: 'primary' })
      return
    }
    mk('div', 'note', p, 'Everyone gets a number of votes to spend on notes, cards and shapes.')
    let per = v.perPerson || 5
    const r = mk('label', 'frow', p); mk('span', '', r, 'Votes each')
    seg(r, [[3, '3'], [5, '5'], [10, '10']], per, (n) => { per = n })
    const row = mk('div', 'frow end', p)
    if (model.all().some((o) => o.votes && Object.keys(o.votes).length)) btn(row, null, 'Clear old votes', () => { clearVotes(); closePop() }, { label: 'Clear votes' })
    btn(row, 'vote', 'Start voting', () => { closePop(); clearVotes(); model.apply([{ op: 'meta', f: { voting: { active: true, perPerson: per, startedAt: new Date().toISOString() } } }]) }, { label: 'Start', cls: 'primary' })
    const res = results(); if (res.length) { mk('div', 'pop-h', p, 'Last results'); resultList(p, res) }
  }
  function clearVotes() { const ops = model.all().filter((o) => o.votes && Object.keys(o.votes).length).map((o) => ({ op: 'set', id: o.id, f: { votes: {} } })); if (ops.length) model.apply(ops) }
  const results = () => model.all().map((o) => ({ o, n: Object.values(o.votes || {}).reduce((a, b) => a + b, 0) })).filter((x) => x.n).sort((a, b) => b.n - a.n)
  function resultList(p, res) {
    const list = mk('div', 'slist', p)
    for (const { o, n } of res.slice(0, 12)) {
      const b = mk('button', 'sres', list); b.type = 'button'
      mk('b', 'vcount', b, String(n)); mk('span', '', b, (o.text || o.title || o.type).replace(/\s+/g, ' ').slice(0, 60) || '(empty)')
      b.addEventListener('click', () => { App.select([o.id]); App.fitTo(R.box(o.id), { max: 1.5, pad: 200 }) })
    }
  }
  function endVoting() {
    const v = model.meta.voting || {}
    model.apply([{ op: 'meta', f: { voting: { active: false, perPerson: v.perPerson || 5, endedAt: new Date().toISOString() } } }])
    const res = results()
    openPop(topright.querySelectorAll('.btn')[3], (p) => { mk('div', 'pop-h', p, 'Results'); if (res.length) resultList(p, res); else mk('div', 'empty', p, 'No votes were cast.') }, { cls: 'wide', side: 'down' })
  }

  // ---------- help ----------
  function helpPop(p) {
    mk('div', 'pop-h', p, 'Keyboard shortcuts')
    const rows = [['V', 'Select'], ['H / Space', 'Pan'], ['N', 'Sticky note'], ['T', 'Text'], ['S · R · O', 'Shape · rectangle · circle'], ['L', 'Connection line'], ['P · ⇧P', 'Pen · highlighter'], ['E', 'Eraser'], ['F', 'Frame'], ['K', 'Card'], ['M', 'Mind map (Tab: child)'], ['C', 'Comment'],
      ['Double-click · Enter', 'Edit, or add a sticky · edit the selected'], ['Esc', 'Stop editing · deselect · close'], ['Delete', 'Delete the selection'], ['⌘A', 'Select all'], ['Right-click · ⇧F10', 'Menu for the selection, or the canvas'], ['Tab · ⇧Tab', 'Next · previous object (on the canvas)'], ['⌘Z · ⇧⌘Z', 'Undo · redo'], ['⌘C · ⌘V · ⌘D', 'Copy · paste · duplicate'], ['⌥⌘C · ⌥⌘V', 'Copy style · paste style'], ['⌘G · ⇧⌘G', 'Group · ungroup'], ['⇧⌘L', 'Lock'], ['] · [', 'Forward · backward'], ['Arrows', 'Nudge (⇧: 24)'], ['⇧⌘. · ⇧⌘,', 'Text bigger · smaller'], ['⌘F', 'Search'], ['⇧1 · ⇧2', 'Fit all · fit selection'], ['⌘ scroll / pinch · ⌘+ ⌘−', 'Zoom'], ['⌘0', 'Zoom to 100%'], ['?', 'This list'], ['⇧G', 'Snap to grid'], ['Alt-drag', 'Duplicate while moving']]
    const grid = mk('div', 'keys', p)
    for (const [k, d] of rows) { mk('kbd', '', grid, k); mk('span', '', grid, d) }
  }

  // ---------- links ----------
  // A link opens in the person's browser through the host (K2 asks the first time per site), or is copied. Text only, never markup.
  const setOn = (id, f) => { const o = model.get(id); if (o && !o.locked) model.apply([{ op: 'set', id, f }]) }
  App.on('link-open', (id, el) => openPop(el, (p) => linkPop(p, id, false), { side: 'down' }))
  function linkPop(p, id, editing) {
    const o = model.get(id); if (!o) return
    mk('div', 'pop-h', p, 'Link')
    if (!editing && o.link) {
      const u = mk('div', 'linkurl', p, o.link)
      const row = mk('div', 'prow', p)
      const open = mk('button', 'btn primary', row); open.type = 'button'; open.dataset.autofocus = '1'; mk('span', 'lbl', open, 'Open')
      open.addEventListener('click', () => g.BoardHost.openLink(o.link).then((r) => {
        if (r === 'opened') closePop(); else if (r === 'declined') App.toast('Not opened'); else if (r !== 'busy') App.toast('Opening links isn’t available here: copy it into your browser')
      }))
      const copy = mk('button', 'btn', row); copy.type = 'button'; mk('span', 'lbl', copy, 'Copy link')
      copy.addEventListener('click', () => {
        const done = () => App.toast('Link copied'); const pick = () => { const r = document.createRange(); r.selectNodeContents(u); const s = getSelection(); s.removeAllRanges(); s.addRange(r); App.toast('Press ⌘C to copy the selected link') }
        try { navigator.clipboard.writeText(o.link).then(done, pick) } catch (_) { pick() }
      })
      if (!o.locked) { const ed = mk('button', 'btn', row); ed.type = 'button'; mk('span', 'lbl', ed, 'Edit'); ed.addEventListener('click', () => { p.textContent = ''; linkPop(p, id, true) }) }
      mk('div', 'hint', p, 'Opens in your browser. The first time for a site, K2 asks you.')
      return
    }
    const input = mk('input', 'field', p); input.type = 'url'; input.placeholder = 'https://…'; input.value = o.link || ''; input.dataset.autofocus = '1'; input.setAttribute('aria-label', 'Link address')
    const err = mk('div', 'err', p)
    const row = mk('div', 'prow', p)
    const save = mk('button', 'btn primary', row); save.type = 'button'; mk('span', 'lbl', save, 'Save')
    const go = () => {
      const v = input.value.trim()
      if (!v) { setOn(id, { link: null }); closePop(); return }
      const u = /^[a-z][a-z0-9+.-]*:/i.test(v) ? v : 'https://' + v   // "example.com" means the web address
      try { S.clean(o.type, { link: u }) } catch (_) { err.textContent = 'Use a web address (http:// or https://)'; return }
      setOn(id, { link: u }); closePop()
    }
    save.addEventListener('click', go); input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); go() } })
    if (o.link) { const rm = mk('button', 'btn', row); rm.type = 'button'; mk('span', 'lbl', rm, 'Remove'); rm.addEventListener('click', () => { setOn(id, { link: null }); closePop() }) }
  }

  // ---------- checklists ----------
  // Each item has its own id (ck-…), so an agent adding or removing items while the panel is open never moves
  // an edit onto the wrong item.
  function checksPop(p, id) {
    mk('div', 'pop-h', p, 'Checklist')
    const box = mk('div', '', p)
    const list = () => { const o = model.get(id); return o && Array.isArray(o.checks) ? o.checks.map((c) => Object.assign({}, c)) : [] }
    const save = (l) => setOn(id, { checks: l.length ? l : null })
    // Whatever is typed counts, even if the panel closes before the field's change event (Escape, a click outside).
    const flush = () => {
      const n = list(); let ch = false
      box.querySelectorAll('.ckrow').forEach((r) => { const t = r.querySelector('.field'); if (!t || t.value === t.dataset.orig) return; const c = n.find((x) => x.id === r.dataset.id); if (c) { c.text = t.value; t.dataset.orig = t.value; ch = true } })
      if (ch) save(n)
    }
    popClose = flush   // openPop calls this when the panel closes
    const at = (n, cid) => n.findIndex((x) => x.id === cid)
    const draw = (focusId) => {
      box.textContent = ''; const l = list(); let focusEl = null
      l.forEach((c, i) => {
        const r = mk('div', 'ckrow', box); r.dataset.id = c.id
        const cb = mk('input', '', r); cb.type = 'checkbox'; cb.checked = !!c.done; cb.setAttribute('aria-label', 'Done')
        const t = mk('input', 'field', r); t.value = c.text; t.dataset.orig = c.text; t.placeholder = 'Item'; t.setAttribute('aria-label', 'Item ' + (i + 1))
        cb.addEventListener('change', () => { flush(); const n = list(); const k = at(n, c.id); if (k >= 0) { n[k].done = cb.checked; save(n) } })
        t.addEventListener('change', flush)
        t.addEventListener('keydown', (e) => {
          if (e.key === 'Enter') {
            e.preventDefault(); flush(); const n = list()
            if (n.length >= S.LIMITS.checks) { App.toast('A checklist holds up to ' + S.LIMITS.checks + ' items'); return }
            const k = at(n, c.id); const nid = S.itemId('ck')
            n.splice(k >= 0 ? k + 1 : n.length, 0, { id: nid, text: '', done: false }); save(n); draw(nid)
          } else if (e.key === 'Backspace' && !t.value) {
            e.preventDefault(); const n = list(); const k = at(n, c.id); if (k >= 0) n.splice(k, 1); save(n); draw(n[Math.max(0, k - 1)] && n[Math.max(0, k - 1)].id)
          }
        })
        const x = mk('button', 'btn', r); x.type = 'button'; x.title = 'Remove item'; x.setAttribute('aria-label', 'Remove item ' + (i + 1)); x.appendChild(icon('close', 14))
        x.addEventListener('click', () => { flush(); const n = list(); const k = at(n, c.id); if (k >= 0) n.splice(k, 1); save(n); draw(n[Math.min(Math.max(0, k), n.length - 1)] && n[Math.min(Math.max(0, k), n.length - 1)].id) })
        if (focusId === c.id) focusEl = t
      })
      const add = mk('button', 'btn', box); add.type = 'button'; add.appendChild(icon('plus', 16)); mk('span', 'lbl', add, 'Add item')
      add.addEventListener('click', () => { flush(); const n = list(); if (n.length >= S.LIMITS.checks) { App.toast('A checklist holds up to ' + S.LIMITS.checks + ' items'); return } const nid = S.itemId('ck'); n.push({ id: nid, text: '', done: false }); save(n); draw(nid) })
      if (!l.length && focusId == null) add.dataset.autofocus = '1'
      if (focusEl) focusEl.focus()
    }
    const first = list()[0]
    draw(first ? first.id : null)
    mk('div', 'hint', p, 'Enter adds the next item. Tick items right on the board.')
  }

  // ---------- image crop ----------
  // Drag the box, or its corners, over the whole picture. Done keeps the picture's scale on the board.
  function cropPop(p, id) {
    const o = model.get(id); if (!o) return
    mk('div', 'pop-h', p, 'Crop')
    // Start on the part of the picture that's showing (the box may be a different shape from the crop, drawn cover-style),
    // so Done with no change leaves the board exactly as it is.
    let old = o.crop || { x: 0, y: 0, w: 1, h: 1 }
    { const bi = R.el(id) && R.el(id).querySelector('img'); const nw = bi && bi.naturalWidth; const nh = bi && bi.naturalHeight
      if (nw && nh) { const k = Math.max(o.w / (old.w * nw), o.h / (old.h * nh)); const vw = o.w / k / nw; const vh = o.h / k / nh; old = { x: old.x + (old.w - vw) / 2, y: old.y + (old.h - vh) / 2, w: vw, h: vh } } }
    let c = Object.assign({}, old); const start = Object.assign({}, old)
    const box = mk('div', 'cropbox', p); const img = mk('img', '', box); img.alt = ''; img.setAttribute('src', o.src)
    // The picture fits the panel (300×360 at most), whatever its shape; the box is exactly its size.
    const size = () => { const nw = img.naturalWidth; const nh = img.naturalHeight; if (!nw || !nh) return; const k = Math.min(300 / nw, 360 / nh); img.style.width = Math.round(nw * k) + 'px'; img.style.height = Math.round(nh * k) + 'px'; box.style.width = img.style.width; place() }
    if (img.complete) setTimeout(size, 0); img.addEventListener('load', size)
    const clip = mk('div', 'cclip', box); const rect = mk('div', 'crect', clip); const hs = ['nw', 'ne', 'sw', 'se'].map((k) => { const h = mk('div', 'ch ' + k, box); h.dataset.k = k; return h })
    const place = () => {
      Object.assign(rect.style, { left: c.x * 100 + '%', top: c.y * 100 + '%', width: c.w * 100 + '%', height: c.h * 100 + '%' })
      for (const h of hs) { h.style.left = (c.x + (h.dataset.k[1] === 'e' ? c.w : 0)) * 100 + '%'; h.style.top = (c.y + (h.dataset.k[0] === 's' ? c.h : 0)) * 100 + '%' }
    }
    place()
    const clamp = (v, a, b) => Math.min(b, Math.max(a, v)); const MIN = 0.05
    box.addEventListener('pointerdown', (e) => {
      const k = e.target.dataset && e.target.dataset.k; if (!k && e.target !== rect) return
      e.preventDefault(); box.setPointerCapture(e.pointerId)
      const r = box.getBoundingClientRect(); const s = Object.assign({}, c); const x0 = e.clientX; const y0 = e.clientY
      const move = (m) => {
        const dx = (m.clientX - x0) / r.width; const dy = (m.clientY - y0) / r.height
        if (!k) { c.x = clamp(s.x + dx, 0, 1 - s.w); c.y = clamp(s.y + dy, 0, 1 - s.h) }
        else {
          if (k[1] === 'w') { const nx = clamp(s.x + dx, 0, s.x + s.w - MIN); c.w = s.w + (s.x - nx); c.x = nx } else c.w = clamp(s.w + dx, MIN, 1 - s.x)
          if (k[0] === 'n') { const ny = clamp(s.y + dy, 0, s.y + s.h - MIN); c.h = s.h + (s.y - ny); c.y = ny } else c.h = clamp(s.h + dy, MIN, 1 - s.y)
        }
        place()
      }
      const up = () => { box.removeEventListener('pointermove', move); box.removeEventListener('pointerup', up) }
      box.addEventListener('pointermove', move); box.addEventListener('pointerup', up)
    })
    const row = mk('div', 'prow', p)
    const done = mk('button', 'btn primary', row); done.type = 'button'; done.dataset.autofocus = '1'; mk('span', 'lbl', done, 'Done')
    const reset = mk('button', 'btn', row); reset.type = 'button'; mk('span', 'lbl', reset, 'Whole picture')
    reset.addEventListener('click', () => { c = { x: 0, y: 0, w: 1, h: 1 }; place() })
    done.addEventListener('click', () => {
      if (['x', 'y', 'w', 'h'].every((q) => Math.abs(c[q] - start[q]) < 1e-4)) { closePop(); return }   // nothing changed: leave it exactly as it is
      const r4 = (v) => Math.round(v * 10000) / 10000; const n = { x: r4(c.x), y: r4(c.y), w: r4(c.w), h: r4(c.h) }
      const whole = n.x === 0 && n.y === 0 && n.w === 1 && n.h === 1
      const bi = (R.el(id) || box).querySelector('img'); const nw = img.naturalWidth || (bi && bi.naturalWidth); const nh = img.naturalHeight || (bi && bi.naturalHeight)
      if (!nw || !nh) { App.toast('The picture hasn’t loaded yet'); return }
      // One scale on the board (px per picture px), as it's drawn now (cover): the new box is the crop's shape at that
      // scale, placed where that part of the picture already is, so nothing jumps, rotated or not.
      const k = Math.max(o.w / (old.w * nw), o.h / (old.h * nh))
      const vw = o.w / k; const vh = o.h / k; const vx = old.x * nw + (old.w * nw - vw) / 2; const vy = old.y * nh + (old.h * nh - vh) / 2   // what's visible now
      const w = n.w * nw * k; const h = n.h * nh * k
      const dx = (n.x * nw + n.w * nw / 2 - (vx + vw / 2)) * k; const dy = (n.y * nh + n.h * nh / 2 - (vy + vh / 2)) * k
      const a = (o.rot || 0) * Math.PI / 180; const cx = o.x + o.w / 2 + dx * Math.cos(a) - dy * Math.sin(a); const cy = o.y + o.h / 2 + dx * Math.sin(a) + dy * Math.cos(a)
      setOn(id, { crop: whole ? null : n, w: Math.max(8, Math.round(w)), h: Math.max(8, Math.round(h)), x: Math.round(cx - w / 2), y: Math.round(cy - h / 2) })
      closePop()
    })
    mk('div', 'hint', p, 'Drag the box or its corners. The picture itself is kept whole.')
  }

  // ---------- history: what changed in the last day, read-only ----------
  function historyPop(p) {
    mk('div', 'pop-h', p, 'Changes in the last day')
    const list = mk('div', 'slist hist', p); mk('div', 'sinfo', list, 'Reading the changes…')
    const since = Date.now() - 864e5
    const ago = (t) => { const m = Math.round((Date.now() - t) / 6e4); return m < 1 ? 'just now' : m < 60 ? m + ' min ago' : Math.round(m / 60) + ' h ago' }
    const label = (o) => { const t = String(o.title || o.text || o.label || '').replace(/\s+/g, ' ').trim(); return (TYPE_WORD[o.type] || 'Item') + (t ? ' “' + t.slice(0, 48) + '”' : '') }
    const show = (rows, note) => {
      list.textContent = ''
      if (!rows.length) mk('div', 'sinfo', list, 'Nothing changed in the last day.')
      const seen = new Set()
      for (const r of rows) {
        if (seen.has(r.id)) continue; seen.add(r.id)   // the latest change per item
        const o = model.get(r.id)
        const b = mk('button', 'sres', list); b.type = 'button'
        mk('span', 'hl', b, r.deleted || !o ? 'A deleted item' : label(o)); mk('span', 'hm', b, (r.by ? model.nameOf(r.by) : 'Someone') + ' · ' + ago(r.at))
        if (o && !r.deleted) b.addEventListener('click', () => { closePop(); App.select([o.id], { raw: true }); App.fitTo(R.box(o.id), { max: 1.5, pad: 200 }) })
        else b.disabled = true
      }
      if (note) mk('div', 'hint', p, note)
    }
    App.store.history(since).then((rows) => {
      if (rows) { show(rows, rows.partial ? 'Showing the most changes it could read; there were more.' : ''); return }
      show(model.all().filter((o) => Date.parse(o.updatedAt) >= since).map((o) => ({ id: o.id, at: Date.parse(o.updatedAt), by: o.updatedBy })).sort((a, b) => b.at - a.at), 'Deleted items aren’t listed here.')
    }, () => { list.textContent = ''; mk('div', 'sinfo', list, 'Couldn’t read the changes just now. Try again in a moment.') })
  }

  // ---------- right-click menu on the canvas ----------
  // On an object: what you can do to the selection. On empty canvas: paste, add, select, zoom. Keys shown are the shortcuts.
  $('viewport').addEventListener('contextmenu', (e) => {
    if (App.editing || App.presenting) return
    e.preventDefault()
    const id = App.objAt(e.target)
    if (id && !App.sel.has(id)) App.select([id])
    if (!id) App.select([])
    canvasMenu(e.clientX, e.clientY, id)
  })
  // The selection's handles sit on the overlay, above the canvas: a right-click there is on the selection.
  $('overlay').addEventListener('contextmenu', (e) => {
    if (App.editing || App.presenting) return
    e.preventDefault()
    const own = e.target.closest && e.target.closest('[data-id]'); const id = own && own.dataset.id   // a connection dot belongs to its object
    if (id && !App.sel.has(id) && model.get(id)) App.select([id])
    canvasMenu(e.clientX, e.clientY, id)
  })
  // From the keyboard (Shift+F10 or the menu key): at the selection, or the middle of the view.
  App.on('canvas-menu', () => {
    if (App.editing || App.presenting) return
    const b = selRect(); const r = $('viewport').getBoundingClientRect()
    canvasMenu(b ? Math.round((b.left + b.right) / 2) : Math.round(r.left + r.width / 2), b ? Math.round((b.top + b.bottom) / 2) : Math.round(r.top + r.height / 2))
  })
  function canvasMenu(cx, cy, target) {   // target: the object right-clicked, if any
    const at = App.toWorld(cx, cy)
    const pt = { left: cx, right: cx, top: cy, bottom: cy, width: 0, height: 0 }
    openPop(pt, (p) => {
      const item = (ic, label, f, key, danger) => { const b = mk('button', 'menuitem' + (danger ? ' danger' : ''), p); b.type = 'button'; b.setAttribute('role', 'menuitem'); b.appendChild(icon(ic, 16)); mk('span', '', b, label); if (key) mk('kbd', '', b, key); b.addEventListener('click', () => { closePop(); f() }) }
      const line = () => mk('div', 'menusep', p)
      const list = App.selected()
      if (list.length) {
        item('copy', 'Copy', () => App.copyCmd(false), '⌘C'); item('copy', 'Duplicate', () => App.duplicate(), '⌘D')
        if (App.hasClip()) item('copy', 'Paste here', () => App.pasteClip(at), '⌘V')
        if (App.styleSource(target)) item('brush', 'Copy style', () => App.copyStyle(target), '⌥⌘C')
        if (App.hasStyle()) item('brush', 'Paste style', () => App.pasteStyle(), '⌥⌘V')
        line()
        item('front', 'Bring to front', () => App.layer('front'), '⌘]'); item('back', 'Send to back', () => App.layer('back'), '⌘[')
        if (App.canTidy()) item('grid', 'Tidy up', () => App.tidy())
        if (list.length > 1 && !list.every((o) => o.group && o.group === list[0].group)) item('group', 'Group', () => App.group(), '⌘G')
        if (list.some((o) => o.group)) item('group', 'Ungroup', () => App.ungroup(), '⇧⌘G')
        item('lock', list.every((o) => o.locked) ? 'Unlock' : 'Lock', () => App.toggleLock(), '⇧⌘L')
        item('fit', 'Zoom to selection', () => App.fitTo(App.selBox(), { max: 2 }), '⇧2')
        if (F.chat && chatAgents.length) item('chat', 'Ask an agent about this…', () => askPop(pt, list))
        line()
        item('trash', 'Delete', () => App.del(), '⌫', true)
      } else {
        if (App.hasClip()) item('copy', 'Paste here', () => App.pasteClip(at), '⌘V')
        // As a click with the tool would place them: centred on the point, default size, straight into typing.
        const addHere = (t, f) => {
          const d = S.DEFAULTS[t]; let x = at[0] - d.w / 2; let y = at[1] - d.h / 2
          if (App.snap) { x = Math.round(x / 24) * 24; y = Math.round(y / 24) * 24 }
          App.startEdit(App.create(t, Object.assign({ x: Math.round(x), y: Math.round(y), w: d.w, h: d.h, text: '' }, f)))
        }
        item('sticky', 'Add a sticky note here', () => addHere('sticky', { color: App.lastSticky || 'yellow' }), 'N')
        item('text', 'Add text here', () => addHere('text', {}), 'T')
        line()
        item('grid', 'Select all', () => App.select(model.all().map((o) => o.id), { raw: true }), '⌘A')
        item('fit', 'Fit everything', () => App.fitAll(), '⇧1')
      }
    }, { menu: true, side: 'point', label: App.sel.size ? 'Selection menu' : 'Canvas menu' })
  }

  // "Ask an agent about this": pick one of the board's agents; K2's message box gets a note naming the selection
  // (type, words, id), for the person to finish and send. Nothing is sent from here.
  function askPop(pt, list) {
    const agents = chatAgents.slice()
    openPop(pt, (p) => {
      mk('div', 'menuhead', p, 'Ask an agent about this')
      mk('div', 'menunote', p, 'Fills its message box (replacing anything typed there); you send it.')
      for (const a of agents) {
        const b = mk('button', 'menuitem', p); b.type = 'button'; b.setAttribute('role', 'menuitem'); avatar(b, a, 'av'); mk('span', '', b, a.label)
        b.addEventListener('click', () => {
          closePop()
          App.store.chat.draft(a.address, askText(list)).then((ok) => { if (!ok) App.toast('Couldn’t open the chat with ' + a.label) })
        })
      }
    }, { menu: true, side: 'point', label: 'Ask an agent' })
  }
  function askText(list) {
    const words = (o) => String(o.title || o.text || o.label || (o.type === 'image' ? o.alt : '') || '').replace(/\s+/g, ' ').trim()
    const one = (o) => { const w = words(o); return '- ' + o.type + (w ? ' “' + (w.length > 60 ? w.slice(0, 59) + '…' : w) + '”' : '') + ' (' + o.id + ')' }
    const lines = list.slice(0, 12).map(one); if (list.length > 12) lines.push('- and ' + (list.length - 12) + ' more')
    return 'About ' + (list.length === 1 ? 'this' : 'these ' + list.length) + ' on the board:\n' + lines.join('\n') + '\n\n'
  }

  // ---------- images ----------
  // A pasted, picked or dropped picture is kept as the original file (stored by the host, never re-encoded);
  // the object holds only its ref. Before an image object is made, the bytes must decode as a picture:
  // anything else (an empty or broken file) is refused with a toast, and nothing is added.
  async function decodes(blob) {
    if (typeof g.createImageBitmap === 'function') {
      let bm = null
      try { bm = await g.createImageBitmap(blob) } catch (_) { bm = null }
      if (bm) { const size = { w: bm.width, h: bm.height }; if (bm.close) bm.close(); return size.w > 0 && size.h > 0 ? size : null }
      return null
    }
    const url = URL.createObjectURL(blob)
    try {
      return await new Promise((resolve) => { const im = new Image(); im.onload = () => resolve(im.naturalWidth && im.naturalHeight ? { w: im.naturalWidth, h: im.naturalHeight } : null); im.onerror = () => resolve(null); im.src = url })
    } finally { URL.revokeObjectURL(url) }
  }
  const NOT_A_PICTURE = 'That file isn’t a picture Board can show: nothing was added'
  const altOf = (name) => String(name || '').replace(/\.[A-Za-z0-9]{1,5}$/, '').replace(/[_-]+/g, ' ').trim()
  function placeImage(ref, size, at, name, n) {
    const w = Math.min(480, size.w); const h = Math.max(1, Math.round(w * size.h / size.w)); const c = at || App.viewCentre()
    const off = (n || 0) * 32
    return App.create('image', { x: Math.round(c[0] - w / 2 + off), y: Math.round(c[1] - h / 2 + off), w, h, src: ref, alt: altOf(name) })
  }
  // A file the page holds (a paste): check it, store it, place it.
  async function addImageFile(file, at, n) {
    if (!App.store.canBlobs()) { App.toast('Pictures can’t be saved on this board here'); return null }
    if (!file || !(file.size > 0)) { App.toast(NOT_A_PICTURE); return null }
    const size = await decodes(file)
    if (!size) { App.toast(NOT_A_PICTURE); return null }
    let info = null
    try { info = await App.store.putBlob(file, file.name) } catch (e) {
      App.toast(e && (e.code === 'too_large' || e.code === 'value_too_large') ? 'That picture is over 16 MB: nothing was added' : 'Couldn’t save that picture (' + ((e && e.code) || 'error') + ')')
      return null
    }
    if (!info || !S.isBlobRef(info.ref)) { App.toast('Couldn’t save that picture'); return null }
    return placeImage(info.ref, size, at, file.name, n)
  }
  // A file the host already stored (picked or dropped): check its bytes decode, then place it.
  async function addStoredImage(f, at, n) {
    if (!f || !S.isBlobRef(f.ref) || !/^image\//.test(String(f.mime || ''))) { App.toast(NOT_A_PICTURE); return null }
    let got = null
    try { got = await App.store.blobGet(f.ref) } catch (e) { App.toast('Couldn’t read that picture (' + ((e && e.code) || 'error') + ')'); return null }
    const size = got && got.blob ? await decodes(got.blob) : null
    if (!size) { App.toast(NOT_A_PICTURE); return null }
    return placeImage(f.ref, size, at, f.name, n)
  }
  App.addImageFile = addImageFile
  App.on('paste-image', (file) => { addImageFile(file) })
  // The toolbar's Picture button: K2's file picker (right after the click).
  App.pickImages = () => host.pick({ accept: ['image/*'], multiple: true }).then(async (r) => {
    const files = (r && r.files) || []
    for (let i = 0; i < files.length; i++) await addStoredImage(files[i], null, i)
  }, (e) => App.toast('Couldn’t open the file picker (' + ((e && e.code) || 'error') + ')'))
  if (host.canPick()) btn(toolbar, 'image', 'Picture: add one from your files', () => App.pickImages())
  // Files dropped on the board: placed where they were dropped.
  host.onDrop((d) => {
    const files = (d && d.files) || []; const at = App.toWorld(d.x, d.y)
    ;(async () => { for (let i = 0; i < files.length; i++) await addStoredImage(files[i], at, i) })()
  })
  // A copy of an older picture (a data: image inside the object) is stored as a file too: the board never
  // makes a new data: image.
  App.on('copied', async (ids) => {
    for (const id of ids) {
      const o = model.get(id); if (!o || o.type !== 'image' || !S.isDataImage(o.src)) continue
      const m = /^data:(image\/[a-z]+);base64,(.*)$/.exec(o.src); const s = atob(m[2]); const bytes = new Uint8Array(s.length)
      for (let i = 0; i < s.length; i++) bytes[i] = s.charCodeAt(i)
      let info = null
      try { info = await App.store.putBlob(new Blob([bytes], { type: m[1] }), null) } catch (e) { info = null }
      if (info && S.isBlobRef(info.ref) && model.get(id)) model.apply([{ op: 'set', id, f: { src: info.ref } }], { noUndo: true })
      else { model.apply([{ op: 'del', id }], { noUndo: true }); App.toast('Couldn’t copy an older picture: it was left out') }
    }
  })

  // ---------- export ----------
  // JSON: K2's own export, not the .k5board file (no K2 ids, pictures inside). PNG: a picture of the board, when the
  // page has K2's PNG encoder. Both go to the person's Downloads through K2, right after the click.
  const fileName = (ext) => (boardName().replace(/[\\/:*?"<>|\u0000-\u001f]+/g, ' ').replace(/\s+/g, ' ').trim() || 'Board') + '.' + ext
  App.exportJson = async () => {
    const src = await App.store.exportSource()
    const env = await g.BoardExport.envelope({ title: boardName(), meta: src.meta, objects: src.objects, people: App.store, blobGet: App.store.blobGet })
    return JSON.stringify(env, null, 2)
  }
  App.exportPng = async () => {
    R.all()
    const box = App.contentBox() || { x: 0, y: 0, w: 400, h: 300 }
    return g.BoardExport.png({
      world: $('world'), box, scheme: document.documentElement.dataset.scheme,
      imageData: async (img) => {
        const ref = img.dataset.key ? img.dataset.key.split('|')[0] : ''
        if (!S.isBlobRef(ref)) return null
        const got = await App.store.blobGet(ref)
        return g.BoardExport.dataUrl(got.blob)
      },
    })
  }
  App.canPng = () => !!g.K2Png
  function exportPop(p) {
    mk('div', 'pop-h', p, 'Export')
    if (!host.canDownload()) { mk('div', 'note', p, 'Saving files isn’t available here.'); return }
    mk('div', 'note', p, 'Save “' + boardName() + '” to your Downloads.')
    const row = mk('div', 'frow end', p)
    const fail = (e) => App.toast(e && e.code === 'newer_format' ? 'A newer K2 made this board, so this K2 can’t export it as JSON. Update K2, or save a picture' : 'Couldn’t export the board (' + ((e && (e.code || e.message)) || 'error') + ')')
    btn(row, 'export', 'Download the board as a JSON file', () => {
      closePop()
      host.download(fileName('json'), () => App.exportJson(), { mime: 'application/json' }).then((r) => { if (r && r.saved === false) App.toast('Not saved') }, fail)
    }, { label: 'JSON' })
    if (App.canPng()) {
      btn(row, 'image', 'Download a picture of the board (PNG)', () => {
        closePop()
        host.download(fileName('png'), () => App.exportPng(), { mime: 'image/png' }).then((r) => { if (r && r.saved === false) App.toast('Not saved') }, fail)
      }, { label: 'PNG' })
    }
  }

  // ---------- refusals: say why, never cut ----------
  const MANY = { 100: 'rows', 30: 'columns', 50: 'items', 200: 'tags', 5000: 'points' }
  App.refusalText = (e) => {
    if (e.code === 'too_long') return 'That’s too long to save: at most ' + Number(e.max).toLocaleString('en') + ' characters'
    if (e.code === 'newer_format') return 'This board is read only: a newer K2 made it. Update K2 to edit it'
    if (e.code === 'too_many') return 'That’s too many to save: at most ' + Number(e.max).toLocaleString('en') + ' ' + (MANY[e.max] || 'entries')
    if (e.code === 'value_too_large') return 'That’s too big to save: one item holds at most ' + Math.round((e.max || S.VALUE_MAX) / 1024) + ' KB'
    if (e.code === 'unknown_field' || e.code === 'unknown_type') return 'That can’t be saved: Board doesn’t know “' + (e.field || 'it') + '”'
    return 'That can’t be saved: ' + (e.field ? '“' + e.field + '” isn’t valid' : 'a value isn’t valid')
  }
  model.onRefuse((e) => {
    if (e.remote) App.toast('A change from elsewhere couldn’t be shown: ' + App.refusalText(e).replace(/^That/, 'it'))
    else App.toast(App.refusalText(e))
  })

  // ---------- theme ----------
  function luminance(css) {
    const m = String(css).match(/\d+(\.\d+)?/g); if (!m || m.length < 3) return 1
    const [r, g2, b] = m.slice(0, 3).map(Number); return (0.2126 * r + 0.7152 * g2 + 0.0722 * b) / 255
  }
  function hexLum(v) { v = String(v || '').trim(); if (/^#[0-9a-f]{6}/i.test(v)) return luminance(parseInt(v.slice(1, 3), 16) + ',' + parseInt(v.slice(3, 5), 16) + ',' + parseInt(v.slice(5, 7), 16)); if (/^#[0-9a-f]{3}$/i.test(v)) return luminance([1, 2, 3].map((i) => parseInt(v[i] + v[i], 16)).join(',')); return luminance(v) }
  function applyScheme(scheme) {
    let dark
    if (scheme === 'dark' || scheme === 'light') dark = scheme === 'dark'
    else { const c = getComputedStyle(document.documentElement).getPropertyValue('--zen-canvas'); dark = c ? hexLum(c) < 0.45 : matchMedia('(prefers-color-scheme: dark)').matches }
    document.documentElement.dataset.scheme = dark ? 'dark' : 'light'
    R.setScheme(dark); updateToolDots(); drawMinimap(); App.drawOverlay()
  }
  App.applyScheme = applyScheme

  // ---------- host: tell it where our floating controls are ----------
  function shareHits() {
    try {
      const rects = ['topleft', 'topright', 'hud', 'ctx', 'pop', 'present', 'toolbar'].map((id) => $(id)).filter((e) => e && !e.hidden && e.offsetWidth).map((e) => e.getBoundingClientRect())
      const t = $('toast'); if (t && !t.hidden && t.classList.contains('act')) rects.push(t.getBoundingClientRect())   // its Undo takes clicks
      g.BoardHost.setHitRegions(rects)
    } catch (_) {}
  }
  addEventListener('resize', () => { shareHits(); drawMinimap(); placeCtx() })

  // Leaving the page (a reload): finish the edit and save now. Just hidden: save, keep editing.
  addEventListener('pagehide', () => { App.endEdit(true); App.store.flushNow() })
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'hidden') App.store.flushNow() })

  // ---------- boot ----------
  const rm = matchMedia('(prefers-reduced-motion: reduce)')
  App.reduced = rm.matches
  if (rm.addEventListener) rm.addEventListener('change', () => { App.reduced = rm.matches || host.reducedMotion() })   // follows the system setting live
  themeNow()
  R.setVoting(!!(model.meta.voting && model.meta.voting.active))
  R.all(); App.countStickies(); drawName(); updateToolDots(); App.setTool('select')
  App.fitAll(true)
  if (!model.all().length) { const r = App.rect(); App.cam = Object.assign(App.cam, { s: 1, x: r.width / 2, y: r.height / 2 }); App.applyCam() }
  drawHud(); App.fire('change', new Set())
  $('viewport').focus({ preventScroll: true })
  emptyHint()

  // Board quick-start hint on an empty board (again after switching to an empty one).
  function emptyHint() {
    if (hint) { hint.remove(); hint = null }
    if (model.all().length) return
    hint = mk('div', 'emptyhint', document.getElementById('app'))
    const art = mk('div', 'art', hint); art.setAttribute('aria-hidden', 'true')
    for (const c of ['n1', 'n2', 'n3']) mk('i', c, art)
    mk('b', '', hint, 'This board is empty')
    mk('span', '', hint, 'Double-click anywhere for a sticky note, pick a tool on the left, or start from a template.')
    const tb = btn(hint, 'template', 'Templates', (el) => openPop(el, templatesPop, { cls: 'wide', side: 'up' }), { label: 'Browse templates', cls: 'primary' })
    tb.style.pointerEvents = 'auto'
  }

  // Pictures follow the zoom: a sharper thumbnail when the board is zoomed in.
  let imageTier = 0
  App.on('camera', () => { const t = Math.round(Math.log2(Math.max(0.05, App.cam.s))); if (t !== imageTier) { imageTier = t; R.refreshImages() } })

  host.start().then(() => {
    App.reduced = App.reduced || host.reducedMotion()
    host.onScheme((s) => { hostScheme = s; themeNow() })
    shareHits()
    if (F.boards && host.canListFiles()) {
      files.on = true
      host.listFiles((list) => {
        if (!list || !Array.isArray(list.boards)) { files.failed = 'bad_answer'; filesChanged(); return }
        files.rows = list.boards; files.truncated = list.truncated === true; files.failed = null
        filesChanged()
      }, (e) => { files.failed = (e && e.code) || 'failed'; filesChanged() })
    }
    App.store.start((batch) => model.applyRemote(batch)).then(() => {
      // People: person:owner is the person at this computer; K2's names for them and the agents.
      model.isMe = App.store.isMe; model.nameOf = App.store.nameOf
      R.setImageSource((ref, px) => App.store.blobUrl(ref, px), () => App.cam.s)
      R.refreshFont()
      // The person's own saves: light/dark and the board they had open.
      const back = App.store.restored()
      if (back && back.scheme && back.scheme !== pickScheme && SCHEMES.some((x) => x[0] === back.scheme)) { pickScheme = back.scheme; themeNow(); drawThemeBtn() }
      if (back && back.board) App.switchBoard(back.board)
      noteActivity(); host.ready()
    }, () => host.ready())
  }, () => { App.toast('Board couldn’t connect: edits won’t save'); host.ready() })
})(window)
