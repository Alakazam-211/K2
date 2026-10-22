// Kanban (k2:kanban@2, the Kanban Garden in New Garden). Columns and cards
// live in this widget's Garden data (k2.store, contract-garden-data-v1),
// never in this code, so the person
// and the agents they allow see one board:
//   - one key per object under the board's prefix P (config.prefix, else
//     board/<garden id>/, so each Kanban Garden is its own board):
//       P col/<id>   {id, name, order}
//       P card/<id>  {id, title, note, col, order}   col = a column's <id>
//       P meta       {v, seeded}   written once, with the starter board
//     Lower order first; a missing or bad field never breaks the board.
//   - field-level writes: a gesture changes some fields; the write carries
//     the rev it read (ifRev), and on `conflict` it reads the key again and
//     re-applies only those fields (an agent's edit survives your drag);
//   - live: k2.store.subscribe shows anyone's change (another window, an
//     agent's `k2 store put --widget k2:kanban …`) with no reload, with the
//     writer's name on the card.
// The starter board (three columns, three cards that explain the board) is
// written once, with `meta`, in one batch. A board you emptied stays empty.
// Text anyone wrote goes in with textContent; notes go through md.js
// (marked + DOMPurify, a DOM fragment, never markup strings).
//
// Moving cards: pointer events (mouse, pen, touch), not HTML5 drag and
// drop: K2's desktop app takes native drops for files, so HTML5 drags never
// land. Press and move 4 px (or hold a touch 300 ms, or touch the grip) and
// the card lifts; Esc puts it back.
(function () {
  'use strict'
  var k2 = window.k2
  var md = window.KanbanMarkdown

  // The data space agents name: `k2 store … --widget k2:kanban`.
  var WIDGET = 'k2:kanban'
  var STEP = 1024
  var MAX_TITLE = 300
  var MAX_NOTE = 20000
  var MAX_NAME = 80
  var BATCH_MAX = 64
  var KEY_CHARS = /^[A-Za-z0-9._~:@+/-]*$/

  // To do / Doing / Done: the plainest Kanban flow, one word each, and
  // nothing assumed about the work. Rename, add or remove them freely.
  var DEFAULT_COLUMNS = [
    { id: 'todo', name: 'To do' },
    { id: 'doing', name: 'Doing' },
    { id: 'done', name: 'Done' }
  ]
  var DEFAULT_CARDS = [
    {
      id: 'start-drag',
      col: 'todo',
      title: 'Drag me to Doing',
      note: 'Drag a card to another column, or up and down to reorder. **Click** a card to edit it.'
    },
    {
      id: 'start-notes',
      col: 'doing',
      title: 'Notes take markdown',
      note: '**Bold**, *italics*, lists and `code`.\n\nDone with these examples? Open one and press **Delete**.'
    },
    {
      id: 'start-agents',
      col: 'done',
      title: 'Your agents can help',
      note: 'Let an agent use this board and it can add, edit and move cards. Its cards show its name. **For agents** at the top says how.'
    }
  ]

  var prefix = ''
  var viewerName = 'You'
  var canWrite = false
  var loaded = false
  /** key → {rev, value, by} */
  var cols = new Map()
  var cards = new Map()
  /** {rev, value} or null */
  var meta = null
  /** key → the rev that deleted it (a late list row never brings it back). */
  var gone = new Map()
  var selectedKey = null
  var wantFocus = null
  var rendering = false
  var renderPending = false
  // Inline editors, kept across redraws (an agent's change redraws the board).
  var composer = null // {col, text}
  var renaming = null // {key, text}
  var addingCol = null // {text}
  var openMenu = null // column key
  // The card in the editor and the fields as it opened.
  var editing = null // {key, base: {title, note, col}}

  function $(id) {
    return document.getElementById(id)
  }

  function el(tag, cls, text) {
    var n = document.createElement(tag)
    if (cls) n.className = cls
    if (text != null) n.textContent = text
    return n
  }

  function button(cls, text, label) {
    var b = el('button', cls, text)
    b.type = 'button'
    if (label) b.setAttribute('aria-label', label)
    return b
  }

  function newId() {
    return Date.now().toString(36) + Math.random().toString(36).slice(2, 7)
  }

  function isObj(v) {
    return v !== null && typeof v === 'object' && !Array.isArray(v)
  }

  function str(v, max) {
    return typeof v === 'string' ? v.slice(0, max) : ''
  }

  function num(v) {
    return typeof v === 'number' && isFinite(v) ? v : null
  }

  function colPrefix() {
    return prefix + 'col/'
  }
  function cardPrefix() {
    return prefix + 'card/'
  }
  function metaKey() {
    return prefix + 'meta'
  }

  function validPrefix(p) {
    if (p === '') return true
    return p.length <= 160 && KEY_CHARS.test(p) && p.charAt(0) !== '/' && p.slice(-1) === '/' &&
      p.indexOf('//') < 0 && p.split('/').indexOf('..') < 0
  }

  // The Garden file's config.prefix, else one board per Garden.
  function boardPrefix() {
    var c = (k2 && k2.config) || {}
    if (typeof c.prefix === 'string' && validPrefix(c.prefix)) return c.prefix
    var g = k2 && k2.widget && typeof k2.widget.garden === 'string' ? k2.widget.garden : ''
    g = g.replace(/[^A-Za-z0-9._-]/g, '').slice(0, 64)
    return 'board/' + (g || 'main') + '/'
  }

  // ── Messages ──────────────────────────────────────────────────────────────
  function banner(text) {
    var b = $('banner')
    b.textContent = text || ''
    b.hidden = !text
  }

  function toast(msg, err) {
    var t = $('toast')
    t.textContent = msg
    t.classList.toggle('err', !!err)
    t.classList.add('show')
    clearTimeout(toast.timer)
    toast.timer = setTimeout(function () { t.classList.remove('show') }, 2600)
  }

  function errText(err) {
    var code = err && err.code
    if (code === 'verb_local') return 'This board lives on the computer that runs K2. Open this Garden there to see it.'
    if (code === 'verb_unavailable' || code === 'unknown_verb' || code === 'not_exposed') {
      return 'This board needs K2 0.45.6 or later (Garden data). Update K2 to use it.'
    }
    if (code === 'cap_not_granted') return 'This board can’t save here.'
    if (code === 'space_full' || code === 'store_full') return 'The board is full. Delete some cards to make room.'
    if (code === 'value_too_large') return 'That card is too long to save. Shorten its note.'
    if (code === 'rate_limited' || code === 'busy') return 'K2 is busy. Try again in a moment.'
    if (code === 'paused') return 'Saving is paused: too many changes in a row. Resume it on the widget.'
    if (code === 'conflict') return 'The board changed while you were saving. Try again.'
    if (code === 'gone') return 'That card was deleted meanwhile.'
    if (code === 'author_unknown' || code === 'not_granted') return 'K2 didn’t give this board its data (' + code + ').'
    return (err && err.message) || String(err)
  }

  function report(err) {
    toast(errText(err), true)
  }

  // ── Authors ───────────────────────────────────────────────────────────────
  // K2 sends `you`, `<name>` or `<name> via <widget>` for the person and
  // `agent:<handle>` for an agent.
  function isAgent(by) {
    return typeof by === 'string' && by.indexOf('agent:') === 0
  }

  function label(by) {
    if (typeof by !== 'string' || !by) return ''
    if (by === 'you') return viewerName
    if (isAgent(by)) return by.slice(6)
    var via = by.indexOf(' via ')
    if (via > 0) {
      var who = by.slice(0, via)
      return who === 'you' ? viewerName : who
    }
    return by
  }

  // ── The board's state ─────────────────────────────────────────────────────
  /** One row from a list or the feed. `value` null or `deleted` = gone. */
  function apply(row) {
    if (!row || typeof row.key !== 'string') return
    var key = row.key
    var rev = typeof row.rev === 'number' ? row.rev : 0
    var dead = gone.get(key)
    if (dead !== undefined && dead >= rev) return
    var deleted = !!row.deleted || row.value === null || row.value === undefined
    if (key === metaKey()) {
      if (meta && meta.rev > rev) return
      meta = deleted ? null : { rev: rev, value: row.value }
      if (deleted) gone.set(key, rev)
      return
    }
    var map = key.indexOf(colPrefix()) === 0 ? cols : key.indexOf(cardPrefix()) === 0 ? cards : null
    if (!map || key.length === (map === cols ? colPrefix() : cardPrefix()).length) return
    var have = map.get(key)
    if (have && have.rev > rev) return
    if (deleted) {
      map.delete(key)
      gone.set(key, rev)
      if (selectedKey === key) selectedKey = null
      if (openMenu === key) openMenu = null
    } else {
      map.set(key, { rev: rev, value: row.value, by: typeof row.by === 'string' ? row.by : '' })
      gone.delete(key)
    }
  }

  function forgetAll() {
    cols.clear()
    cards.clear()
    gone.clear()
    meta = null
  }

  function byOrder(a, b) {
    var ao = a.order === null ? Infinity : a.order
    var bo = b.order === null ? Infinity : b.order
    if (ao !== bo) return ao < bo ? -1 : 1
    return a.key < b.key ? -1 : a.key > b.key ? 1 : 0
  }

  /** Columns, left to right. */
  function columnList() {
    var out = []
    cols.forEach(function (e, key) {
      var v = isObj(e.value) ? e.value : {}
      out.push({
        key: key,
        id: key.slice(colPrefix().length),
        name: str(v.name, MAX_NAME).trim() || 'Untitled',
        order: num(v.order),
        by: e.by,
        rev: e.rev
      })
    })
    return out.sort(byOrder)
  }

  /** Cards per column id, top to bottom. A card whose column is gone (or
   *  never named one) shows in the first column until someone moves it. */
  function cardsByColumn(columns) {
    var out = {}
    columns.forEach(function (c) { out[c.id] = [] })
    var first = columns.length ? columns[0].id : null
    cards.forEach(function (e, key) {
      var v = isObj(e.value) ? e.value : {}
      var col = typeof v.col === 'string' && out[v.col] ? v.col : first
      if (col === null) return
      out[col].push({
        key: key,
        id: key.slice(cardPrefix().length),
        title: str(v.title, MAX_TITLE).trim() || 'Untitled',
        note: str(v.note, MAX_NOTE),
        col: col,
        order: num(v.order),
        by: e.by,
        rev: e.rev
      })
    })
    Object.keys(out).forEach(function (id) { out[id].sort(byOrder) })
    return out
  }

  function cardsIn(colId) {
    return cardsByColumn(columnList())[colId] || []
  }

  // ── Writes ────────────────────────────────────────────────────────────────
  function k2Error(code, message) {
    var e = new Error(message || code)
    e.code = code
    return e
  }

  /** Write `fields` onto the object at `key` (field-level): ifRev = the rev
   *  read; on conflict read it again and re-apply only these fields. With
   *  `create` false, a key someone deleted meanwhile stays deleted. */
  async function change(map, key, fields, create) {
    var batch = newId()
    for (var attempt = 0; attempt < 3; attempt++) {
      var have = map.get(key)
      if (!have && !create) throw k2Error('gone')
      var base = have && isObj(have.value) ? have.value : {}
      var value = Object.assign({}, base, fields, { batch: batch })
      try {
        var r = await k2.store.put(key, value, { ifRev: have ? have.rev : 0 })
        apply({ key: key, rev: r && typeof r.rev === 'number' ? r.rev : 0, value: value, by: 'you' })
        render()
        return
      } catch (err) {
        if (!err || err.code !== 'conflict') throw err
        var now = await k2.store.get(key)
        if (now) apply(now)
        else if (have) apply({ key: key, rev: have.rev + 1, deleted: true })
        render()
      }
    }
    throw k2Error('conflict', 'Someone else keeps changing this; try again.')
  }

  async function remove(map, key) {
    for (var attempt = 0; attempt < 3; attempt++) {
      var have = map.get(key)
      if (!have) return
      try {
        var r = await k2.store.delete(key, { ifRev: have.rev })
        apply({ key: key, rev: r && typeof r.rev === 'number' ? r.rev : have.rev + 1, deleted: true })
        render()
        return
      } catch (err) {
        if (!err || err.code !== 'conflict') throw err
        var now = await k2.store.get(key)
        if (now) apply(now)
        else apply({ key: key, rev: have.rev + 1, deleted: true })
      }
    }
    throw k2Error('conflict')
  }

  /** Several writes as one rev, all or nothing: a batch, or a transaction
   *  past 64. */
  async function runOps(ops) {
    if (!ops.length) return null
    var r
    if (ops.length <= BATCH_MAX || typeof k2.store.tx !== 'function') {
      r = await k2.store.batch(ops)
    } else {
      var tx = k2.store.tx()
      try {
        await tx.add(ops)
        r = await tx.commit()
      } catch (err) {
        tx.abort().catch(function () {})
        throw err
      }
    }
    var rev = r && typeof r.rev === 'number' ? r.rev : 0
    ops.forEach(function (op) {
      if (op.op === 'delete') apply({ key: op.key, rev: rev, deleted: true })
      else apply({ key: op.key, rev: rev, value: op.value, by: 'you' })
    })
    render()
    return r
  }

  function putOp(map, key, value) {
    var have = map.get(key)
    return { op: 'put', key: key, value: value, ifRev: have ? have.rev : 0 }
  }

  function delOp(map, key) {
    var have = map.get(key)
    return { op: 'delete', key: key, ifRev: have ? have.rev : 0 }
  }

  // The starter board, once: `meta` is written with it (ifRev 0, so two
  // windows can't both seed), and a board that already has anything (an
  // agent got there first) only gets `meta`.
  async function seed() {
    if (meta || !canWrite) return
    var batch = newId()
    var ops = [{ op: 'put', key: metaKey(), value: { v: 1, seeded: true, batch: batch }, ifRev: 0 }]
    if (cols.size === 0 && cards.size === 0) {
      DEFAULT_COLUMNS.forEach(function (c, i) {
        ops.push({ op: 'put', key: colPrefix() + c.id, value: { id: c.id, name: c.name, order: (i + 1) * STEP, batch: batch }, ifRev: 0 })
      })
      DEFAULT_CARDS.forEach(function (c) {
        ops.push({
          op: 'put',
          key: cardPrefix() + c.id,
          value: { id: c.id, title: c.title, note: c.note, col: c.col, order: STEP, batch: batch },
          ifRev: 0
        })
      })
    }
    try {
      await runOps(ops)
    } catch (err) {
      if (!err || err.code !== 'conflict') throw err
      // Another window seeded first: its rows arrive on the feed.
      await loadAll()
      render()
    }
  }

  /** The order for a slot between `prev` and `next` (either may be null),
   *  or null when the column needs renumbering. */
  function orderBetween(prev, next) {
    if ((prev && prev.order === null) || (next && next.order === null)) return null
    if (!prev && !next) return STEP
    if (!prev) return next.order - STEP
    if (!next) return prev.order + STEP
    var mid = (prev.order + next.order) / 2
    if (!(mid > prev.order && mid < next.order) || next.order - prev.order < 1e-6) return null
    return mid
  }

  /** Put card `key` at `index` of column `colId` (index among the column's
   *  other cards). */
  async function placeCard(key, colId, index) {
    var have = cards.get(key)
    if (!have) return
    var all = cardsIn(colId)
    var list = all.filter(function (c) { return c.key !== key })
    if (index < 0) index = 0
    if (index > list.length) index = list.length
    var at = all.findIndex(function (c) { return c.key === key })
    if (at !== -1 && at === index) return
    selectedKey = key
    var order = orderBetween(list[index - 1] || null, list[index] || null)
    if (order !== null) return change(cards, key, { col: colId, order: order }, false)
    // No room between the neighbours (or an agent left out `order`):
    // renumber the column in one batch.
    var keys = list.map(function (c) { return c.key })
    keys.splice(index, 0, key)
    var batch = newId()
    var ops = keys.map(function (k, i) {
      var e = cards.get(k)
      var v = Object.assign({}, isObj(e.value) ? e.value : {}, { col: colId, order: (i + 1) * STEP, batch: batch })
      return putOp(cards, k, v)
    })
    return runOps(ops)
  }

  async function moveColumn(key, dir) {
    var list = columnList()
    var at = list.findIndex(function (c) { return c.key === key })
    var to = at + dir
    if (at < 0 || to < 0 || to >= list.length) return
    var rest = list.filter(function (c) { return c.key !== key })
    var order = orderBetween(rest[to - 1] || null, rest[to] || null)
    if (order !== null) return change(cols, key, { order: order }, false)
    var keys = rest.map(function (c) { return c.key })
    keys.splice(to, 0, key)
    var batch = newId()
    return runOps(keys.map(function (k, i) {
      var e = cols.get(k)
      return putOp(cols, k, Object.assign({}, isObj(e.value) ? e.value : {}, { order: (i + 1) * STEP, batch: batch }))
    }))
  }

  function endOrder(list) {
    var max = null
    list.forEach(function (c) { if (c.order !== null && (max === null || c.order > max)) max = c.order })
    return (max === null ? 0 : max) + STEP
  }

  async function addCard(colId, title) {
    var id = newId()
    var t = String(title || '').trim().slice(0, MAX_TITLE)
    if (!t) return
    await change(cards, cardPrefix() + id, { id: id, title: t, note: '', col: colId, order: endOrder(cardsIn(colId)) }, true)
  }

  async function addColumn(name) {
    var n = String(name || '').trim().slice(0, MAX_NAME)
    if (!n) return
    var id = newId()
    await change(cols, colPrefix() + id, { id: id, name: n, order: endOrder(columnList()) }, true)
  }

  async function deleteColumn(key) {
    var list = columnList()
    var col = list.find(function (c) { return c.key === key })
    if (!col) return
    var inside = cardsByColumn(list)[col.id] || []
    // Cards that only show here because their own column is gone go with
    // this column's choice too.
    if (!inside.length) {
      var ok = await ask({
        title: 'Delete “' + col.name + '”?',
        text: 'The column is empty.',
        actions: [{ id: 'delete', label: 'Delete column', kind: 'danger' }]
      })
      if (ok === 'delete') await remove(cols, key)
      return
    }
    var others = list.filter(function (c) { return c.key !== key })
    var pick = null
    var extra = null
    var actions = []
    if (others.length) {
      extra = el('label', 'field', 'Move its cards to ')
      pick = el('select')
      pick.id = 'confirmMoveTo'
      others.forEach(function (c) {
        var o = el('option', null, c.name)
        o.value = c.id
        pick.appendChild(o)
      })
      extra.appendChild(pick)
      actions.push({ id: 'move', label: 'Move cards, delete column', kind: 'primary' })
    }
    actions.push({ id: 'delete', label: 'Delete column and cards', kind: 'danger' })
    var n = inside.length
    var choice = await ask({
      title: 'Delete “' + col.name + '”?',
      text: 'It has ' + n + (n === 1 ? ' card.' : ' cards.') + (others.length ? ' Move them to another column, or delete them with it.' : ' They go with it.'),
      extra: extra,
      actions: actions
    })
    if (choice !== 'move' && choice !== 'delete') return
    var batch = newId()
    var ops = []
    if (choice === 'move') {
      var dest = pick.value
      var order = endOrder(cardsByColumn(list)[dest] || [])
      inside.forEach(function (c, i) {
        var e = cards.get(c.key)
        ops.push(putOp(cards, c.key, Object.assign({}, isObj(e.value) ? e.value : {}, { col: dest, order: order + i * STEP, batch: batch })))
      })
    } else {
      inside.forEach(function (c) { ops.push(delOp(cards, c.key)) })
    }
    ops.push(delOp(cols, key))
    await runOps(ops)
  }

  // ── Drawing ───────────────────────────────────────────────────────────────
  function filterText() {
    return ($('search').value || '').trim().toLowerCase()
  }

  function matches(card, q) {
    return !q || (card.title + ' ' + card.note).toLowerCase().indexOf(q) !== -1
  }

  // A redraw waits while a card is lifted or a pointer is down, so the
  // button being clicked (or the card being pressed) isn't swapped out from
  // under the click; it runs right after the release.
  // A release the frame never hears (the pointer left it) holds nothing up
  // for long: after HOLD_MS the redraw runs anyway.
  var HOLD_MS = 1500
  var pointerDown = false
  var downAt = 0
  function flush() {
    if (renderPending) render()
  }
  document.addEventListener('pointerdown', function () {
    pointerDown = true
    downAt = Date.now()
  }, true)
  function released() {
    pointerDown = false
    if (renderPending) setTimeout(flush, 0)
  }
  document.addEventListener('pointerup', released, true)
  document.addEventListener('pointercancel', released, true)

  function render() {
    if (drag || (pointerDown && Date.now() - downAt < HOLD_MS)) {
      if (!drag && !renderPending) setTimeout(flush, HOLD_MS + 50)
      renderPending = true
      return
    }
    renderPending = false
    var board = $('board')
    var active = document.activeElement
    var refocus = wantFocus || (active && board.contains(active) && active.dataset ? active.dataset.focus || null : null)
    wantFocus = null
    rendering = true
    try {
      board.replaceChildren()
      var columns = columnList()
      var byCol = cardsByColumn(columns)
      var q = filterText()
      columns.forEach(function (c, i) { board.appendChild(columnEl(c, byCol[c.id], q, i, columns.length)) })
      if (canWrite && loaded) board.appendChild(addColumnEl(columns.length === 0))
      else if (loaded && !columns.length) board.appendChild(el('p', 'board-empty', 'This board has no columns.'))
      if (refocus) {
        var f = board.querySelector('[data-focus="' + cssEscape(refocus) + '"]')
        if (f) {
          f.focus()
          if (typeof f.setSelectionRange === 'function' && typeof f.value === 'string') f.setSelectionRange(f.value.length, f.value.length)
        }
      }
    } finally {
      rendering = false
    }
    if (editing && !cards.has(editing.key)) $('edBy').textContent = 'This card was deleted.'
  }

  function cssEscape(s) {
    return String(s).replace(/["\\]/g, '\\$&')
  }

  function columnEl(c, list, q, index, total) {
    var col = el('section', 'col')
    col.dataset.key = c.key
    col.dataset.col = c.id
    col.setAttribute('aria-label', c.name)
    var head = el('header', 'col-head')
    if (renaming && renaming.key === c.key) {
      head.appendChild(inlineInput('col-rename', renaming.text, MAX_NAME, 'rename:' + c.key, 'Column name', {
        input: function (v) { renaming.text = v },
        commit: commitRename,
        cancel: function () { renaming = null; render() }
      }))
    } else {
      var h = el('h2', 'col-name', c.name)
      h.title = c.name
      if (canWrite) h.addEventListener('dblclick', function () { startRename(c) })
      head.appendChild(h)
    }
    var visible = list.filter(function (card) { return matches(card, q) })
    head.appendChild(el('span', 'count', q ? visible.length + '/' + list.length : String(list.length)))
    if (canWrite) {
      var mb = button('col-menu-btn', '⋯', 'Column actions for ' + c.name)
      mb.dataset.focus = 'menu:' + c.key
      mb.setAttribute('aria-haspopup', 'menu')
      mb.setAttribute('aria-expanded', openMenu === c.key ? 'true' : 'false')
      mb.addEventListener('click', function (e) {
        e.stopPropagation()
        openMenu = openMenu === c.key ? null : c.key
        render()
      })
      head.appendChild(mb)
      if (openMenu === c.key) head.appendChild(menuEl(c, index, total))
    }
    col.appendChild(head)
    var body = el('div', 'col-body')
    body.dataset.col = c.id
    visible.forEach(function (card) { body.appendChild(cardEl(card)) })
    if (!visible.length) body.appendChild(el('div', 'empty', q ? 'No matches' : canWrite ? 'Drop cards here' : 'No cards'))
    col.appendChild(body)
    if (canWrite) col.appendChild(composerEl(c))
    return col
  }

  function menuEl(c, index, total) {
    var m = el('div', 'col-menu')
    m.setAttribute('role', 'menu')
    function item(text, on, disabled, danger) {
      var b = button('col-menu-item' + (danger ? ' danger' : ''), text)
      b.setAttribute('role', 'menuitem')
      b.disabled = !!disabled
      b.addEventListener('click', function (e) {
        e.stopPropagation()
        openMenu = null
        render()
        Promise.resolve().then(on).catch(report)
      })
      m.appendChild(b)
    }
    item('Rename', function () { startRename(c) })
    item('Move left', function () { return moveColumn(c.key, -1) }, index === 0)
    item('Move right', function () { return moveColumn(c.key, 1) }, index === total - 1)
    item('Delete column…', function () { return deleteColumn(c.key) }, false, true)
    return m
  }

  /** A text box that survives redraws: Enter commits, Esc cancels, blur
   *  commits (but not the blur a redraw causes). */
  function inlineInput(cls, value, max, focusId, labelText, on, multiline) {
    var inp = el(multiline ? 'textarea' : 'input', cls)
    if (!multiline) inp.type = 'text'
    else inp.rows = 2
    inp.value = value
    inp.maxLength = max
    inp.dataset.focus = focusId
    inp.setAttribute('aria-label', labelText)
    inp.addEventListener('input', function () { on.input(inp.value) })
    inp.addEventListener('keydown', function (e) {
      if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
        e.preventDefault()
        on.commit(true)
      } else if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        on.cancel()
      }
    })
    inp.addEventListener('blur', function () {
      if (!rendering) on.commit(false)
    })
    return inp
  }

  function startRename(c) {
    renaming = { key: c.key, text: c.name }
    wantFocus = 'rename:' + c.key
    render()
  }

  function commitRename() {
    if (!renaming) return
    var r = renaming
    renaming = null
    var name = r.text.trim().slice(0, MAX_NAME)
    var have = cols.get(r.key)
    var old = have && isObj(have.value) ? str(have.value.name, MAX_NAME).trim() : ''
    render()
    if (!name || name === old || !have) return
    change(cols, r.key, { name: name }, false).catch(report)
  }

  function composerEl(c) {
    var wrap = el('div', 'col-foot')
    if (composer && composer.col === c.id) {
      wrap.appendChild(inlineInput('add-card-input', composer.text, MAX_TITLE, 'compose:' + c.id, 'New card title in ' + c.name, {
        input: function (v) { composer.text = v },
        commit: function (byEnter) {
          var text = composer ? composer.text : ''
          if (byEnter) {
            // Stay open for the next card.
            composer.text = ''
            wantFocus = 'compose:' + c.id
          } else {
            composer = null
          }
          render()
          if (text.trim()) addCard(c.id, text).catch(report)
        },
        cancel: function () { composer = null; render() }
      }, true))
      var hint = el('div', 'foot-hint', 'Enter adds · Esc closes')
      wrap.appendChild(hint)
    } else {
      var b = button('add-card', '+ Add a card')
      b.dataset.focus = 'add:' + c.id
      b.addEventListener('click', function () {
        composer = { col: c.id, text: '' }
        wantFocus = 'compose:' + c.id
        render()
      })
      wrap.appendChild(b)
    }
    return wrap
  }

  function addColumnEl(empty) {
    var wrap = el('section', 'col add-col')
    if (addingCol) {
      wrap.appendChild(inlineInput('add-col-input', addingCol.text, MAX_NAME, 'addcol', 'New column name', {
        input: function (v) { addingCol.text = v },
        commit: function () {
          var text = addingCol ? addingCol.text : ''
          addingCol = null
          render()
          if (text.trim()) addColumn(text).catch(report)
        },
        cancel: function () { addingCol = null; render() }
      }))
    } else {
      if (empty) wrap.appendChild(el('p', 'board-empty', 'No columns yet.'))
      var b = button('add-column', '+ Add a column')
      b.id = 'addColumn'
      b.dataset.focus = 'addcol-btn'
      b.addEventListener('click', function () {
        addingCol = { text: '' }
        wantFocus = 'addcol'
        render()
      })
      wrap.appendChild(b)
    }
    return wrap
  }

  function cardEl(card) {
    var a = el('article', 'card')
    a.dataset.key = card.key
    a.dataset.col = card.col
    a.dataset.focus = 'card:' + card.key
    a.tabIndex = 0
    a.setAttribute('aria-label', card.title)
    if (selectedKey === card.key) a.classList.add('selected')
    if (isAgent(card.by)) a.classList.add('by-agent')
    var grip = el('div', 'grip')
    grip.setAttribute('aria-hidden', 'true')
    a.appendChild(grip)
    var main = el('div', 'card-main')
    main.appendChild(el('h3', 'card-title', card.title))
    if (card.note) {
      var note = el('div', 'card-note')
      if (md) md.fill(note, card.note)
      else note.textContent = card.note
      main.appendChild(note)
    }
    var by = label(card.by)
    if (by) main.appendChild(el('div', 'card-by', by))
    a.appendChild(main)
    if (canWrite) a.addEventListener('pointerdown', onCardPointerDown)
    a.addEventListener('click', function (e) {
      e.stopPropagation()
      // The click after a drag can land on the card the drop replaced.
      if (swallowClick || !a.isConnected) return
      selectedKey = card.key
      openEditor(card.key)
    })
    a.addEventListener('keydown', onCardKey)
    return a
  }

  function onCardKey(e) {
    var key = e.currentTarget.dataset.key
    if (e.target !== e.currentTarget) return
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      openEditor(key)
      return
    }
    if (!canWrite || e.altKey || e.metaKey || e.ctrlKey || e.shiftKey) return
    var columns = columnList()
    var byCol = cardsByColumn(columns)
    var ci = -1
    var idx = -1
    columns.forEach(function (c, i) {
      var at = byCol[c.id].findIndex(function (x) { return x.key === key })
      if (at !== -1) { ci = i; idx = at }
    })
    if (ci < 0) return
    var colId = columns[ci].id
    var p = null
    if (e.key === 'ArrowUp') p = idx > 0 ? placeCard(key, colId, idx - 1) : null
    else if (e.key === 'ArrowDown') p = placeCard(key, colId, idx + 1)
    else if (e.key === 'ArrowLeft') p = ci > 0 ? placeCard(key, columns[ci - 1].id, 0) : null
    else if (e.key === 'ArrowRight') p = ci < columns.length - 1 ? placeCard(key, columns[ci + 1].id, 0) : null
    else return
    e.preventDefault()
    wantFocus = 'card:' + key
    if (p) p.catch(report)
  }

  // ── Pointer drag (mouse, pen, touch) ──────────────────────────────────────
  // Press a card and move 4 px (mouse / pen), or touch the grip, or hold a
  // touch still for 300 ms, and the card lifts: a ghost follows the pointer,
  // the placeholder line shows where it lands, a column (or the board) under
  // the pointer near its edge scrolls, Esc or a cancelled pointer puts it
  // back. Release over a column to drop it there.
  var DRAG_SLOP = 4
  var TOUCH_SLOP = 8
  var TOUCH_HOLD_MS = 300
  var EDGE = 40
  var MAX_SPEED = 18
  var press = null // a pointer that is down on a card, not yet dragging
  var drag = null // the live drag
  var swallowClick = false

  function onCardPointerDown(e) {
    // A release K2 never passed on (pointer left the frame) leaves no stale
    // press or drag behind: a new press clears it.
    if (drag) { finishDrag(false); return }
    if (press) endPress()
    if (e.pointerType === 'mouse' && e.button !== 0) return
    var card = e.currentTarget
    press = {
      key: card.dataset.key,
      card: card,
      pointerId: e.pointerId,
      type: e.pointerType || 'mouse',
      x: e.clientX,
      y: e.clientY,
      timer: null
    }
    if (press.type === 'touch') {
      if (e.target.closest('.grip')) {
        e.preventDefault()
        beginDrag(e.clientX, e.clientY)
      } else {
        press.timer = setTimeout(function () {
          if (press) beginDrag(press.x, press.y)
        }, TOUCH_HOLD_MS)
      }
    }
    window.addEventListener('pointermove', onPointerMove, true)
    window.addEventListener('pointerup', onPointerUp, true)
    window.addEventListener('pointercancel', onPointerCancel, true)
  }

  function beginDrag(x, y) {
    var p = press
    if (!p) return
    if (p.timer) clearTimeout(p.timer)
    press = null
    var card = p.card
    var box = card.getBoundingClientRect()
    var ghost = card.cloneNode(true)
    ghost.classList.add('drag-ghost')
    ghost.classList.remove('selected', 'dragging')
    ghost.removeAttribute('tabindex')
    ghost.removeAttribute('data-focus')
    ghost.setAttribute('aria-hidden', 'true')
    ghost.style.width = box.width + 'px'
    ghost.style.left = box.left + 'px'
    ghost.style.top = box.top + 'px'
    document.body.appendChild(ghost)
    selectedKey = p.key
    document.querySelectorAll('.card.selected').forEach(function (c) { c.classList.remove('selected') })
    card.classList.add('dragging', 'selected')
    document.body.classList.add('card-dragging')
    try { card.setPointerCapture(p.pointerId) } catch (err) { /* the frame may not own the pointer */ }
    drag = {
      key: p.key,
      card: card,
      pointerId: p.pointerId,
      ghost: ghost,
      dx: x - box.left,
      dy: y - box.top,
      x: x,
      y: y,
      body: null,
      raf: null
    }
    window.addEventListener('keydown', onDragKey, true)
    document.addEventListener('touchmove', blockTouchScroll, { passive: false, capture: true })
    track(x, y)
    autoScrollTick()
  }

  function onPointerMove(e) {
    if (press && e.pointerId === press.pointerId) {
      var dist = Math.hypot(e.clientX - press.x, e.clientY - press.y)
      if (press.type === 'touch') {
        // Moving before the hold ends is a scroll, not a drag.
        if (dist > TOUCH_SLOP) endPress()
        return
      }
      if (dist < DRAG_SLOP) return
      beginDrag(press.x, press.y)
    }
    if (!drag || e.pointerId !== drag.pointerId) return
    e.preventDefault()
    track(e.clientX, e.clientY)
  }

  function onPointerUp(e) {
    if (press && e.pointerId === press.pointerId) {
      endPress()
      return
    }
    if (!drag || e.pointerId !== drag.pointerId) return
    e.preventDefault()
    track(e.clientX, e.clientY)
    finishDrag(true)
  }

  function onPointerCancel(e) {
    if (press && e.pointerId === press.pointerId) endPress()
    else if (drag && e.pointerId === drag.pointerId) finishDrag(false)
  }

  // While a card is lifted, keys don't reach the board; Esc puts it back.
  function onDragKey(e) {
    if (!drag) return
    e.stopImmediatePropagation()
    if (e.key !== 'Escape') return
    e.preventDefault()
    finishDrag(false)
  }

  function blockTouchScroll(e) {
    if (drag) e.preventDefault()
  }

  function endPress() {
    if (press && press.timer) clearTimeout(press.timer)
    press = null
    if (!drag) unlisten()
  }

  function unlisten() {
    window.removeEventListener('pointermove', onPointerMove, true)
    window.removeEventListener('pointerup', onPointerUp, true)
    window.removeEventListener('pointercancel', onPointerCancel, true)
    window.removeEventListener('keydown', onDragKey, true)
    document.removeEventListener('touchmove', blockTouchScroll, { capture: true })
  }

  function placeholder() {
    var p = $('ph')
    if (!p) {
      p = el('div', 'placeholder')
      p.id = 'ph'
    }
    return p
  }

  function cardAfter(container, y) {
    var list = [].slice.call(container.querySelectorAll('.card:not(.dragging)'))
    var closest = { offset: Number.NEGATIVE_INFINITY, el: null }
    list.forEach(function (child) {
      var box = child.getBoundingClientRect()
      var offset = y - box.top - box.height / 2
      if (offset < 0 && offset > closest.offset) closest = { offset: offset, el: child }
    })
    return closest.el
  }

  function siblingCard(node, dir) {
    var n = dir === 'next' ? node.nextElementSibling : node.previousElementSibling
    while (n && (n.id === 'ph' || n.classList.contains('dragging') || !n.classList.contains('card'))) {
      n = dir === 'next' ? n.nextElementSibling : n.previousElementSibling
    }
    return n
  }

  // The column body under a point (the header or any part of a column counts).
  function bodyAt(x, y) {
    var hit = document.elementFromPoint(x, y)
    if (!hit || !hit.closest) return null
    var body = hit.closest('.col-body')
    if (body) return body
    var col = hit.closest('.col')
    return col ? col.querySelector('.col-body') : null
  }

  function track(x, y) {
    var d = drag
    if (!d) return
    d.x = x
    d.y = y
    d.ghost.style.left = (x - d.dx) + 'px'
    d.ghost.style.top = (y - d.dy) + 'px'
    var body = bodyAt(x, y)
    if (body !== d.body) {
      if (d.body) d.body.classList.remove('over')
      d.body = body
      if (body) body.classList.add('over')
    }
    var ph = placeholder()
    if (!body) {
      if (ph.parentNode) ph.parentNode.removeChild(ph)
      return
    }
    var empty = body.querySelector('.empty')
    if (empty) empty.remove()
    var after = cardAfter(body, y)
    if (after) {
      if (after.previousElementSibling !== ph) body.insertBefore(ph, after)
    } else if (body.lastElementChild !== ph) {
      body.appendChild(ph)
    }
  }

  // Speed for a point near an edge: 0 outside the band, up to MAX_SPEED.
  function edgeSpeed(pos, lo, hi) {
    var band = Math.min(EDGE, (hi - lo) / 3)
    if (!(band > 0)) return 0
    if (pos < lo + band) return -Math.ceil(MAX_SPEED * Math.min(1, (lo + band - pos) / band))
    if (pos > hi - band) return Math.ceil(MAX_SPEED * Math.min(1, (pos - (hi - band)) / band))
    return 0
  }

  function scrollBy(node, axis, v) {
    if (!node || !v) return false
    if (axis === 'y') {
      var max = node.scrollHeight - node.clientHeight
      if (max <= 0 || (v < 0 && node.scrollTop <= 0) || (v > 0 && node.scrollTop >= max)) return false
      node.scrollTop = Math.max(0, Math.min(max, node.scrollTop + v))
      return true
    }
    var maxX = node.scrollWidth - node.clientWidth
    if (maxX <= 0 || (v < 0 && node.scrollLeft <= 0) || (v > 0 && node.scrollLeft >= maxX)) return false
    node.scrollLeft = Math.max(0, Math.min(maxX, node.scrollLeft + v))
    return true
  }

  // One auto-scroll step: the column under the pointer first, then the board.
  function autoScrollStep() {
    var d = drag
    if (!d) return false
    var moved = false
    var wrap = $('boardWrap')
    if (d.body) {
      var r = d.body.getBoundingClientRect()
      moved = scrollBy(d.body, 'y', edgeSpeed(d.y, r.top, r.bottom)) || moved
    }
    if (wrap) {
      var w = wrap.getBoundingClientRect()
      if (!moved) moved = scrollBy(wrap, 'y', edgeSpeed(d.y, w.top, w.bottom)) || moved
      moved = scrollBy(wrap, 'x', edgeSpeed(d.x, w.left, w.right)) || moved
    }
    if (moved) track(d.x, d.y)
    return moved
  }

  var nextFrame = typeof window.requestAnimationFrame === 'function'
    ? function (fn) { return window.requestAnimationFrame(fn) }
    : function (fn) { return setTimeout(fn, 16) }
  var cancelFrame = typeof window.cancelAnimationFrame === 'function'
    ? function (h) { window.cancelAnimationFrame(h) }
    : function (h) { clearTimeout(h) }

  function autoScrollTick() {
    if (!drag) return
    autoScrollStep()
    drag.raf = nextFrame(autoScrollTick)
  }

  function finishDrag(commit) {
    var d = drag
    if (!d) return
    drag = null
    if (d.raf != null) cancelFrame(d.raf)
    unlisten()
    try { d.card.releasePointerCapture(d.pointerId) } catch (err) { /* already released */ }
    if (d.ghost.parentNode) d.ghost.parentNode.removeChild(d.ghost)
    document.body.classList.remove('card-dragging')
    if (d.body) d.body.classList.remove('over')
    // The click that follows this release is not a card click.
    swallowClick = true
    setTimeout(function () { swallowClick = false }, 0)
    var ph = $('ph')
    var target = null
    if (commit && d.body && ph && ph.parentNode === d.body) {
      var colId = d.body.dataset.col
      // The slot among the column's other cards (all of them, not only
      // the ones a filter shows).
      var list = cardsIn(colId).filter(function (c) { return c.key !== d.key })
      var next = siblingCard(ph, 'next')
      var prev = siblingCard(ph, 'prev')
      var index = list.length
      var find = function (k) { return list.findIndex(function (c) { return c.key === k }) }
      if (next && find(next.dataset.key) !== -1) index = find(next.dataset.key)
      else if (prev && find(prev.dataset.key) !== -1) index = find(prev.dataset.key) + 1
      target = { col: colId, index: index }
    }
    if (ph && ph.parentNode) ph.parentNode.removeChild(ph)
    wantFocus = 'card:' + d.key
    render()
    if (target) placeCard(d.key, target.col, target.index).catch(report)
  }

  window.addEventListener('click', function (e) {
    if (!swallowClick) return
    swallowClick = false
    e.preventDefault()
    e.stopPropagation()
  }, true)

  // No native drags anywhere on the board (they never land in K2's app).
  document.addEventListener('dragstart', function (e) {
    if (e.target && e.target.closest && e.target.closest('#board')) e.preventDefault()
  })

  document.addEventListener('click', function (e) {
    if (openMenu && !(e.target.closest && e.target.closest('.col-menu, .col-menu-btn'))) {
      openMenu = null
      render()
    }
  })

  document.addEventListener('keydown', function (e) {
    if (e.key !== 'Escape') return
    if (openMenu) {
      openMenu = null
      render()
    } else if (!$('editor').hidden) {
      closeEditor()
    } else if (!$('textPanel').hidden) {
      $('textPanel').hidden = true
    }
  })

  $('search').addEventListener('input', render)

  // ── Dialogs ───────────────────────────────────────────────────────────────
  // In-page panels: a sealed widget has no confirm() or modal dialogs.
  var asking = null

  /** {title, text, extra?, actions: [{id, label, kind}]} → the action's id,
   *  or null for Cancel / Esc. */
  function ask(opts) {
    if (asking) asking(null)
    return new Promise(function (resolve) {
      var box = $('confirm')
      $('confirmTitle').textContent = opts.title
      $('confirmText').textContent = opts.text || ''
      var extra = $('confirmExtra')
      extra.replaceChildren()
      if (opts.extra) extra.appendChild(opts.extra)
      var row = $('confirmActions')
      row.replaceChildren()
      function done(v) {
        asking = null
        box.hidden = true
        document.removeEventListener('keydown', onKey, true)
        resolve(v)
      }
      function onKey(e) {
        if (e.key === 'Escape') {
          e.preventDefault()
          e.stopPropagation()
          done(null)
        }
      }
      var cancel = button('btn', 'Cancel')
      cancel.addEventListener('click', function () { done(null) })
      row.appendChild(cancel)
      opts.actions.forEach(function (a) {
        var b = button('btn ' + (a.kind || ''), a.label)
        b.dataset.action = a.id
        b.addEventListener('click', function () { done(a.id) })
        row.appendChild(b)
      })
      asking = done
      document.addEventListener('keydown', onKey, true)
      box.hidden = false
      cancel.focus()
    })
  }

  function openEditor(key) {
    var e = cards.get(key)
    if (!e) return
    var v = isObj(e.value) ? e.value : {}
    var columns = columnList()
    var resolved = cardsByColumn(columns)
    var colId = ''
    Object.keys(resolved).forEach(function (id) {
      if (resolved[id].some(function (c) { return c.key === key })) colId = id
    })
    editing = { key: key, base: { title: str(v.title, MAX_TITLE), note: str(v.note, MAX_NOTE), col: colId } }
    $('editorTitle').textContent = canWrite ? 'Edit card' : 'Card'
    $('edTitle').value = editing.base.title
    $('edNote').value = editing.base.note
    var sel = $('edCol')
    sel.replaceChildren()
    columns.forEach(function (c) {
      var o = el('option', null, c.name)
      o.value = c.id
      sel.appendChild(o)
    })
    sel.value = colId
    var by = label(e.by)
    $('edBy').textContent = by ? 'Last changed by ' + by : ''
    ;['edTitle', 'edNote', 'edCol'].forEach(function (id) { $(id).disabled = !canWrite })
    $('edSave').hidden = !canWrite
    $('edDelete').hidden = !canWrite
    $('editor').hidden = false
    $('edTitle').focus()
  }

  function closeEditor() {
    var key = editing && editing.key
    editing = null
    $('editor').hidden = true
    if (key) {
      wantFocus = 'card:' + key
      render()
    }
  }

  async function saveEditor() {
    if (!editing || !canWrite) return
    var ed = editing
    var title = ($('edTitle').value || '').trim().slice(0, MAX_TITLE) || 'Untitled'
    var note = ($('edNote').value || '').slice(0, MAX_NOTE)
    var col = $('edCol').value
    var fields = {}
    if (title !== ed.base.title.trim()) fields.title = title
    if (note !== ed.base.note) fields.note = note
    if (col && col !== ed.base.col) {
      fields.col = col
      fields.order = endOrder(cardsIn(col))
    }
    closeEditor()
    if (!Object.keys(fields).length) return
    await change(cards, ed.key, fields, false)
  }

  $('editorForm').addEventListener('submit', function (e) {
    e.preventDefault()
    saveEditor().catch(report)
  })
  $('edCancel').addEventListener('click', closeEditor)
  $('edDelete').addEventListener('click', function () {
    if (!editing) return
    var key = editing.key
    var title = ($('edTitle').value || '').trim() || 'this card'
    $('editor').hidden = true
    ask({ title: 'Delete “' + title + '”?', text: 'This can’t be undone from the board.', actions: [{ id: 'delete', label: 'Delete card', kind: 'danger' }] })
      .then(function (v) {
        if (v !== 'delete') {
          $('editor').hidden = false
          return
        }
        editing = null
        return remove(cards, key)
      })
      .catch(report)
  })

  // ── Export, import, agents ────────────────────────────────────────────────
  function boardTitle() {
    var n = k2 && k2.widget && typeof k2.widget.name === 'string' ? k2.widget.name.trim() : ''
    return n || 'Kanban'
  }

  function escTitle(t) {
    return t.replace(/\\/g, '\\\\').replace(/\*/g, '\\*')
  }

  /** The board as markdown: `## Column`, then `- **Title**` with the note
   *  indented under it. Import reads the same shape. */
  function exportMarkdown() {
    var columns = columnList()
    var byCol = cardsByColumn(columns)
    var lines = ['# ' + boardTitle(), '']
    columns.forEach(function (c) {
      lines.push('## ' + c.name)
      var list = byCol[c.id]
      if (!list.length) lines.push('_(no cards)_')
      list.forEach(function (card) {
        lines.push('- **' + escTitle(card.title) + '**')
        if (card.note.trim()) {
          card.note.replace(/\s+$/, '').split(/\r?\n/).forEach(function (l) { lines.push(l ? '  ' + l : '') })
        }
      })
      lines.push('')
    })
    return lines.join('\n').replace(/\n+$/, '\n')
  }

  /** Markdown → [{name, cards: [{title, note}]}]: `##` headings are columns;
   *  top-level `-`, `*` or `1.` items are cards (`**Title** — note` too);
   *  lines indented under an item are its note. */
  function parseImport(text) {
    var out = []
    var cur = null
    var card = null
    String(text || '').split(/\r?\n/).forEach(function (line) {
      var h = /^#{2,3}\s+(.+?)\s*#*\s*$/.exec(line)
      if (h) {
        cur = { name: h[1].trim().slice(0, MAX_NAME), cards: [] }
        out.push(cur)
        card = null
        return
      }
      if (/^#\s/.test(line)) return
      var item = /^ ?(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s+)?(.*)$/.exec(line)
      if (item && cur) {
        var t = item[1].trim()
        var bold = /^\*\*(.+?)\*\*\s*(?:[—–:-]\s*(.*))?$/.exec(t)
        var title = (bold ? bold[1] : t).replace(/\\([\\*])/g, '$1').trim().slice(0, MAX_TITLE)
        card = { title: title || 'Untitled', note: bold && bold[2] ? bold[2] : '' }
        cur.cards.push(card)
        return
      }
      if (card && /^(\s{2,}|\t)/.test(line)) {
        card.note += (card.note ? '\n' : '') + line.replace(/^( {2}|\t)/, '')
        return
      }
      if (card && !line.trim()) {
        if (card.note) card.note += '\n'
        return
      }
      card = null
    })
    out.forEach(function (c) {
      c.cards.forEach(function (x) { x.note = x.note.replace(/\s+$/, '').slice(0, MAX_NOTE) })
    })
    return out.filter(function (c) { return c.name })
  }

  /** Adds the parsed columns and cards: a column with the same name gets
   *  the cards; nothing on the board is changed or deleted. */
  async function importBoard(parsed) {
    var columns = columnList()
    var byName = {}
    columns.forEach(function (c) { byName[c.name.toLowerCase()] = c })
    var byCol = cardsByColumn(columns)
    var nextColOrder = endOrder(columns)
    var batch = newId()
    var ops = []
    var added = 0
    parsed.forEach(function (pc) {
      var c = byName[pc.name.toLowerCase()]
      var colId
      var order
      if (c) {
        colId = c.id
        order = endOrder(byCol[c.id] || [])
      } else {
        colId = newId()
        ops.push({ op: 'put', key: colPrefix() + colId, value: { id: colId, name: pc.name, order: nextColOrder, batch: batch }, ifRev: 0 })
        nextColOrder += STEP
        byName[pc.name.toLowerCase()] = { id: colId }
        byCol[colId] = []
        order = STEP
      }
      pc.cards.forEach(function (card) {
        var id = newId() + added
        ops.push({ op: 'put', key: cardPrefix() + id, value: { id: id, title: card.title, note: card.note, col: colId, order: order, batch: batch }, ifRev: 0 })
        order += STEP
        added++
      })
    })
    await runOps(ops)
    return added
  }

  function agentHelp() {
    var columns = columnList()
    var lines = [
      'This board is Garden data on this computer: widget ' + WIDGET + ', keys under ' + prefix,
      '',
      '  ' + prefix + 'col/<id>    {"name": "To do", "order": 1024}',
      '  ' + prefix + 'card/<id>   {"title": "…", "note": "markdown", "col": "<column id>", "order": 2048}',
      '',
      'Columns sort left to right and cards top to bottom by order (lowest first).',
      'Columns now: ' + (columns.length ? columns.map(function (c) { return c.id + ' (' + c.name + ')' }).join(', ') : 'none'),
      '',
      'Let an agent work here (run this outside K2, or use Settings → Gardens → Widget data):',
      '  k2 zen data grant ' + WIDGET + ' --agent <handle> --level write --prefix ' + prefix,
      '',
      'Then the agent can:',
      '  k2 store list --widget ' + WIDGET + ' --prefix ' + prefix,
      '  k2 store put --widget ' + WIDGET + ' ' + prefix + 'card/<new-id> --value \'{"title":"Write the brief","col":"' +
        (columns.length ? columns[0].id : 'todo') + '","order":4096}\' --if-rev 0',
      '  k2 store get --widget ' + WIDGET + ' ' + prefix + 'card/<id>    (change it with put --if-rev <rev>)',
      '  k2 store rm --widget ' + WIDGET + ' ' + prefix + 'card/<id>',
      '',
      'Changes show on the board at once, with the agent’s name on its cards.'
    ]
    return lines.join('\n')
  }

  // The sealed frame may have no clipboard; the caller shows the text
  // selected when neither way works.
  function copyText(text) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      return navigator.clipboard.writeText(text).then(function () { return true }, function () { return fallbackCopy(text) })
    }
    return Promise.resolve(fallbackCopy(text))
  }

  function fallbackCopy(text) {
    var ta = el('textarea')
    ta.value = text
    ta.setAttribute('readonly', '')
    ta.style.position = 'fixed'
    ta.style.left = '-9999px'
    document.body.appendChild(ta)
    ta.select()
    var ok = false
    try { ok = document.execCommand('copy') } catch (e) { ok = false }
    ta.remove()
    return ok
  }

  var panelMode = null // 'export' | 'import' | 'agents'

  function openPanel(mode, title, hint, body) {
    panelMode = mode
    $('textTitle').textContent = title
    $('textHint').textContent = hint
    var ta = $('textBody')
    ta.value = body
    ta.readOnly = mode !== 'import'
    $('textGo').textContent = mode === 'import' ? 'Add to board' : 'Copy'
    $('textGo').hidden = mode === 'import' && !canWrite
    $('textPanel').hidden = false
    ta.focus()
    if (mode !== 'import') ta.select()
  }

  $('textClose').addEventListener('click', function () { $('textPanel').hidden = true })
  $('textGo').addEventListener('click', function () {
    var ta = $('textBody')
    if (panelMode === 'import') {
      var parsed = parseImport(ta.value)
      if (!parsed.length) {
        toast('No “## Column” headings found in that text', true)
        return
      }
      $('textPanel').hidden = true
      importBoard(parsed).then(function (added) {
        toast('Added ' + added + (added === 1 ? ' card' : ' cards'))
      }, report)
      return
    }
    copyText(ta.value).then(function (ok) {
      if (ok) toast('Copied')
      else {
        ta.focus()
        ta.select()
        toast('Clipboard blocked: the text is selected, press Cmd+C', true)
      }
    })
  })

  $('btnExport').addEventListener('click', function () {
    var text = exportMarkdown()
    copyText(text).then(function (ok) {
      if (ok) toast('Copied the board as markdown')
      else openPanel('export', 'Export', 'The board as markdown. It’s selected: press Cmd+C.', text)
    })
  })

  $('btnImport').addEventListener('click', function () {
    openPanel('import', 'Import', 'Paste markdown: “## Column” headings, then “- Card” lines (indented lines are the note). Cards are added; nothing is deleted.', '')
  })

  $('btnAgents').addEventListener('click', function () {
    openPanel('agents', 'For agents', 'Your agents reach this board with k2 store, inside the grant you give them.', agentHelp())
  })

  // ── The top band ──────────────────────────────────────────────────────────
  // On a full-canvas Garden K2's top band (the window's drag strip with its
  // ⋯ menu) floats over this frame. The toolbar sits in that band; its
  // buttons are holes in it (k2.canvas.setHitRegions), the gaps stay the
  // window's drag area. In a column Garden this is a plain header row.
  function shareToolbar() {
    if (!k2 || typeof k2.can !== 'function' || !k2.can('canvas.setHitRegions')) return
    var rects = []
    var nodes = $('toolbar').children
    for (var i = 0; i < nodes.length; i++) {
      if (nodes[i].hidden) continue
      var r = nodes[i].getBoundingClientRect()
      if (r.width > 0 && r.height > 0) rects.push({ x: r.left, y: r.top, width: r.width, height: r.height })
    }
    k2.canvas.setHitRegions(rects).catch(function () {})
  }
  window.addEventListener('resize', shareToolbar)

  // ── Start ─────────────────────────────────────────────────────────────────
  async function loadAll() {
    var seen = new Set()
    var after
    do {
      var page = await k2.store.list({ prefix: prefix, after: after, limit: 500 })
      var items = page && Array.isArray(page.items) ? page.items : []
      items.forEach(function (row) {
        seen.add(row.key)
        apply(row)
      })
      after = page && page.next ? page.next : undefined
    } while (after)
    ;[cols, cards].forEach(function (map) {
      Array.from(map.keys()).forEach(function (key) { if (!seen.has(key)) map.delete(key) })
    })
    if (meta && !seen.has(metaKey())) meta = null
  }

  function onFeed(changes) {
    if (changes && changes.resync) {
      forgetAll()
      ;(changes.items || []).forEach(apply)
    } else if (Array.isArray(changes)) {
      changes.forEach(apply)
    }
    render()
  }

  // For dock.js: the board's text, a toast and the toolbar's holes.
  window.KanbanBoard = {
    exportMarkdown: exportMarkdown,
    toast: toast,
    shareToolbar: shareToolbar,
    garden: function () { return k2 && k2.widget && typeof k2.widget.garden === 'string' ? k2.widget.garden : '' }
  }

  render()

  if (!k2) {
    banner('K2 didn’t start this widget.')
    return
  }
  // K2 connects the frame after this script runs: wait before k2.can and
  // k2.config. Calls wait on their own.
  k2.connected.then(async function () {
    prefix = boardPrefix()
    shareToolbar()
    if (!k2.store || !k2.can('store.list')) {
      banner('This board keeps its cards in Garden data, which needs K2 0.45.6 or later on the computer that runs K2.')
      loaded = true
      render()
      k2.ready()
      return
    }
    canWrite = k2.can('store.put') && k2.can('store.batch')
    document.body.classList.toggle('read-only', !canWrite)
    try {
      var me = await k2.viewer()
      if (me && me.name) viewerName = me.name
    } catch (err) {
      // The name is only a label; keep "You".
    }
    try {
      await loadAll()
      loaded = true
      k2.store.subscribe(prefix, onFeed, function (err) { banner('Live updates stopped: ' + errText(err)) })
      await seed()
      if (!canWrite) banner('This board is read-only here.')
    } catch (err) {
      loaded = true
      banner(errText(err))
    }
    render()
    k2.ready()
  }, function () {
    banner('K2 didn’t start this widget.')
  })
})()
