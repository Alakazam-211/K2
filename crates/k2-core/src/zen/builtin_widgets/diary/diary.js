// k2:diary@1 — the Diary (prd-zen-user-widgets-v2 UWB24; Rosson
// 2026-10-07). One agent at a time on a parchment page. What you write
// soaks into the page; the reply is written out in handwriting at a
// natural pace (replies arrive whole, so the Diary animates the reveal
// itself), capped near 6 s, and a tap shows it all. Pages turn back
// through the history. A contents page with search and ribbons scales to
// any number of agents. Working = an ink shimmer; needs you = a lifted
// bookmark. Reduced motion turns the drawing off.
//
// Rules this file keeps (the k2-zen skill's): agent text only ever goes
// through textContent; no inline handlers; no network or storage.
(function () {
  'use strict'

  var REVEAL_MIN_MS = 900
  var REVEAL_MAX_MS = 6000
  var REVEAL_MS_PER_CHAR = 38
  var OLDER_PAGE = 50
  var RIBBONS = ['#8e2b2b', '#2b5a8e', '#2f6b3a', '#7a4f1d', '#5b2b7a', '#1d6b6b', '#8e6b1d', '#6b1d4f']

  var $ = function (id) {
    var el = document.getElementById(id)
    if (!el) throw new Error('diary: missing #' + id)
    return el
  }
  var dom = {
    root: document.documentElement,
    ribbon: $('ribbon'),
    contents: $('contents'),
    title: $('title'),
    search: $('search'),
    toc: $('toc'),
    tocEmpty: $('toc-empty'),
    leaf: $('leaf'),
    back: $('back'),
    agent: $('agent'),
    where: $('where'),
    bookmark: $('bookmark'),
    sheet: $('sheet'),
    date: $('date'),
    entries: $('entries'),
    shimmer: $('shimmer'),
    note: $('note'),
    pen: $('pen'),
    ink: $('ink'),
    soak: $('soak'),
    hint: $('hint'),
    send: $('send'),
    prev: $('prev'),
    folio: $('folio'),
    next: $('next'),
  }

  var k2 = window.k2
  var state = {
    rows: [],
    query: '',
    open: null, // address
    view: null, // the last ThreadView for `open`
    older: [], // items read with beforeSeq, oldest first
    hasMoreOlder: false,
    loadingOlder: false,
    page: -1, // index into exchanges(); -1 = the last
    unsubThread: null,
    seenSeq: -1, // replies above this are drawn in handwriting
    revealing: null, // {el, text, start, ms, frame}
    absorbedIds: {}, // your messages sent from this page: shown faded
    sending: false,
    reduced: false,
    readyDone: false,
  }

  // ── small helpers ────────────────────────────────────────────────────

  function text(el, s) {
    el.textContent = s == null ? '' : String(s)
    return el
  }

  function make(tag, cls, s) {
    var el = document.createElement(tag)
    if (cls) el.className = cls
    if (s != null) el.textContent = String(s)
    return el
  }

  function hash(s) {
    var h = 2166136261
    for (var i = 0; i < s.length; i++) {
      h ^= s.charCodeAt(i)
      h = Math.imul(h, 16777619)
    }
    return h >>> 0
  }

  function ribbonFor(address) {
    return RIBBONS[hash(address) % RIBBONS.length]
  }

  function reduced() {
    return state.reduced || (k2 && k2.motion && k2.motion.reduced === true)
  }

  function when(at) {
    if (typeof at !== 'number' || !isFinite(at)) return ''
    var ms = at > 1e12 ? at : at * 1000
    var d = new Date(ms)
    try {
      return d.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' })
    } catch (_e) {
      return d.toDateString()
    }
  }

  function rowFor(address) {
    for (var i = 0; i < state.rows.length; i++) if (state.rows[i].address === address) return state.rows[i]
    return null
  }

  function stateWord(row) {
    if (row.state === 'unreachable' || row.activity === 'unreachable') return 'out of reach'
    if (row.needsYou || row.activity === 'needs-you') return 'needs you'
    if (row.working || row.activity === 'working') return 'writing…'
    if (row.activity === 'monitoring') return 'watching'
    if (row.state !== 'ok') return row.stateLabel || row.state
    return ''
  }

  function isWorking(row, view) {
    var turn = view && view.turn
    if (turn && (turn.state === 'working' || turn.state === 'monitoring')) return true
    return !!(row && (row.working || row.activity === 'working'))
  }

  function needsYou(row, view) {
    var turn = view && view.turn
    if (turn && turn.state === 'needs-you') return true
    return !!(row && (row.needsYou || row.activity === 'needs-you'))
  }

  function showError(where, err) {
    var code = err && err.code ? err.code : 'failed'
    var msg = err && err.message ? err.message : String(err)
    if (code === 'cap_not_granted') return text(where, 'This Diary isn’t allowed to do that yet.')
    if (code === 'sending_off') return text(where, 'Sending is turned off for this Diary.')
    if (code === 'not_bound') return text(where, 'That agent isn’t in this Diary’s scope.')
    if (code === 'rate_limited') return text(where, 'Too fast. Wait a moment and try again.')
    return text(where, 'Something went wrong: ' + msg)
  }

  // ── a flourish drawn with perfect-freehand when K2 provides it ────────

  function freehand() {
    var pf = window.PerfectFreehand || window.perfectFreehand
    if (pf && typeof pf.getStroke === 'function') return pf.getStroke
    if (typeof window.getStroke === 'function') return window.getStroke
    return null
  }

  function flourish(seed) {
    var getStroke = freehand()
    var svgNs = 'http://www.w3.org/2000/svg'
    var svg = document.createElementNS(svgNs, 'svg')
    svg.setAttribute('class', 'flourish')
    svg.setAttribute('viewBox', '0 0 120 14')
    svg.setAttribute('aria-hidden', 'true')
    var path = document.createElementNS(svgNs, 'path')
    var pts = []
    var s = seed % 7
    for (var x = 2; x <= 118; x += 4) pts.push([x, 7 + Math.sin((x + s * 9) / 13) * 3.2, 0.5])
    var d
    if (getStroke) {
      var outline = getStroke(pts, { size: 2.6, thinning: 0.65, smoothing: 0.6, streamline: 0.5 })
      d = outline.length ? 'M' + outline.map(function (p) { return p[0].toFixed(1) + ' ' + p[1].toFixed(1) }).join(' L') + ' Z' : ''
    } else {
      d = 'M2 7 ' + pts.map(function (p) { return 'L' + p[0] + ' ' + p[1].toFixed(1) }).join(' ') +
        ' L118 8 ' + pts.slice().reverse().map(function (p) { return 'L' + p[0] + ' ' + (p[1] + 1.2).toFixed(1) }).join(' ') + ' Z'
    }
    path.setAttribute('d', d)
    svg.appendChild(path)
    return svg
  }

  // ── the contents page ────────────────────────────────────────────────

  function filtered() {
    var q = state.query.trim().toLowerCase()
    if (!q) return state.rows
    return state.rows.filter(function (r) {
      return r.label.toLowerCase().indexOf(q) !== -1 || (r.server || '').toLowerCase().indexOf(q) !== -1
    })
  }

  function drawContents() {
    var rows = filtered()
    dom.toc.textContent = ''
    rows.forEach(function (r) {
      var li = make('li')
      li.tabIndex = r.openable ? 0 : -1
      li.setAttribute('role', 'button')
      if (!r.openable) li.classList.add('off')
      if (isWorking(r, null)) li.classList.add('working')
      if (needsYou(r, null)) li.classList.add('needs')
      var tab = make('span', 'tab')
      tab.style.background = ribbonFor(r.address)
      li.appendChild(tab)
      li.appendChild(make('span', 'name', r.label))
      li.appendChild(make('span', 'dots'))
      li.appendChild(make('span', 'state', stateWord(r) || (r.server ? r.server : '')))
      li.title = r.detail || r.label
      var go = function () {
        if (r.openable) openAgent(r.address)
      }
      li.addEventListener('click', go)
      li.addEventListener('keydown', function (e) {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          go()
        }
      })
      dom.toc.appendChild(li)
    })
    dom.tocEmpty.hidden = rows.length > 0
    if (!rows.length) {
      text(dom.tocEmpty, state.rows.length ? 'No agent matches “' + state.query.trim() + '”.' : 'No agents to write to yet.')
    }
  }

  // ── exchanges: one page per message you wrote and what came back ─────

  function allItems() {
    var seen = {}
    var out = []
    var live = (state.view && state.view.items) || []
    state.older.concat(live).forEach(function (it) {
      if (!it || !it.doc || seen[it.id]) return
      seen[it.id] = true
      out.push(it)
    })
    out.sort(function (a, b) { return a.seq - b.seq })
    return out
  }

  function isMine(doc) {
    return doc.via === 'compose'
  }

  function exchanges() {
    var out = []
    var cur = null
    allItems().forEach(function (it) {
      if (isMine(it.doc) || !cur) {
        cur = { items: [], at: it.doc.created_at }
        out.push(cur)
      }
      cur.items.push(it)
    })
    if (!out.length) out.push({ items: [], at: null })
    return out
  }

  function pageIndex(list) {
    return state.page < 0 || state.page >= list.length ? list.length - 1 : state.page
  }

  function entryFor(it, latest) {
    var doc = it.doc
    var mine = isMine(doc)
    var el = make('div', 'entry ' + (mine ? 'mine' : 'agent'))
    el.dataset.id = it.id
    if (mine && latest && state.absorbedIds[it.id]) el.classList.add('absorbed')
    if (doc.kind === 'secret') {
      el.appendChild(make('span', 'from', 'asks for a secret'))
      el.appendChild(document.createTextNode('Answer it in K2’s Thread; the Diary never handles secrets.'))
      return el
    }
    if (!mine) el.appendChild(make('span', 'from', doc.from || ''))
    var body = make('span', 'body')
    var words = doc.body || (doc.choice && doc.choice.prompt) || ''
    body.textContent = words
    el.appendChild(body)
    if (doc.choice && doc.choice.options && doc.choice.options.length) {
      var row = make('div', 'choice')
      var answered = doc.choice.status !== 'pending'
      doc.choice.options.forEach(function (o) {
        var b = make('button', null, o.label)
        b.type = 'button'
        b.disabled = !!answered || !can('thread.answer')
        if (answered && doc.choice.answer === o.label) b.textContent = '✓ ' + o.label
        b.addEventListener('click', function () {
          b.disabled = true
          k2.thread.answer(state.open, it.id, o.label).catch(function (e) {
            b.disabled = false
            dom.note.hidden = false
            showError(dom.note, e)
          })
        })
        row.appendChild(b)
      })
      el.appendChild(row)
    }
    return el
  }

  function can(verb) {
    return !!(k2 && typeof k2.can === 'function' && k2.can(verb))
  }

  // ── handwriting: reveal a reply at a natural pace ────────────────────

  function stopReveal(finish) {
    var r = state.revealing
    if (!r) return
    state.revealing = null
    if (r.frame) cancelAnimationFrame(r.frame)
    if (finish) r.el.textContent = r.text
    if (r.entry) r.entry.classList.remove('writing')
  }

  // `from` keeps the pen where it was when the page is redrawn mid-reply
  // (a later push, such as the turn ending, redraws the page).
  function reveal(entry, from) {
    var body = entry.querySelector('.body')
    if (!body) return
    var full = body.textContent || ''
    if (!full || reduced()) return
    stopReveal(true)
    var ms = from ? from.ms : Math.max(REVEAL_MIN_MS, Math.min(REVEAL_MAX_MS, full.length * REVEAL_MS_PER_CHAR))
    var r = { id: entry.dataset.id, el: body, entry: entry, text: full, start: from ? from.start : performance.now(), ms: ms, frame: 0 }
    state.revealing = r
    body.textContent = ''
    entry.classList.add('writing')
    var step = function (now) {
      if (state.revealing !== r) return
      var t = Math.min(1, (now - r.start) / r.ms)
      // Ease out: the pen slows a little at the end of a thought.
      var shown = Math.ceil(full.length * (1 - Math.pow(1 - t, 1.6)))
      body.textContent = full.slice(0, shown)
      if (t >= 1) return stopReveal(true)
      r.frame = requestAnimationFrame(step)
    }
    r.frame = requestAnimationFrame(step)
  }

  // ── the agent page ───────────────────────────────────────────────────

  function drawLeaf(opts) {
    var row = rowFor(state.open)
    var view = state.view
    var list = exchanges()
    var idx = pageIndex(list)
    var latest = idx === list.length - 1
    var ex = list[idx]
    text(dom.agent, row ? row.label : state.open)
    text(dom.where, row ? [row.server || 'this computer', stateWord(row)].filter(Boolean).join(' · ') : '')
    dom.bookmark.hidden = !needsYou(row, view)
    text(dom.date, when(ex.at))
    // A reply still being written carries on where the pen was.
    var keep = state.revealing
    if (keep) {
      if (keep.frame) cancelAnimationFrame(keep.frame)
      state.revealing = null
    }
    dom.entries.textContent = ''
    var revealEl = null
    var keepEl = null
    ex.items.forEach(function (it, i) {
      if (i === 0 && !isMine(it.doc) && idx === 0) dom.entries.appendChild(flourish(hash(state.open)))
      var el = entryFor(it, latest)
      dom.entries.appendChild(el)
      if (latest && !isMine(it.doc) && it.seq > state.seenSeq && it.doc.kind !== 'secret') revealEl = el
      if (keep && latest && it.id === keep.id) keepEl = el
    })
    if (latest) {
      var items = allItems()
      if (items.length) state.seenSeq = Math.max(state.seenSeq, items[items.length - 1].seq)
    }
    if (keepEl && !(revealEl && revealEl !== keepEl && opts && opts.reveal)) reveal(keepEl, keep)
    else if (revealEl && opts && opts.reveal) reveal(revealEl)
    dom.shimmer.hidden = !(latest && isWorking(row, view))
    var phase = view ? view.phase : 'opening'
    dom.note.hidden = true
    if (phase === 'opening') {
      dom.note.hidden = false
      text(dom.note, 'Opening…')
    } else if (phase !== 'ready') {
      dom.note.hidden = false
      text(dom.note, (view && view.note) || 'This agent can’t be reached right now.')
    } else if (!ex.items.length) {
      dom.note.hidden = false
      text(dom.note, 'A blank page. Write something.')
    }
    var canWrite = latest && phase === 'ready' && can('thread.post') && !!(row && row.openable)
    dom.pen.hidden = !canWrite
    dom.send.disabled = state.sending
    dom.prev.disabled = idx === 0 && !state.hasMoreOlder && !(view && view.hasMore)
    dom.next.disabled = latest
    text(dom.folio, 'page ' + (idx + 1) + ' of ' + list.length)
    dom.ribbon.style.background = ribbonFor(state.open)
  }

  function turn(cls) {
    if (reduced()) return
    dom.leaf.classList.remove('turning', 'back')
    void dom.leaf.offsetWidth
    dom.leaf.classList.add('turning')
    if (cls) dom.leaf.classList.add(cls)
  }

  function showContents() {
    closeThread()
    state.open = null
    dom.leaf.hidden = true
    dom.contents.hidden = false
    dom.ribbon.style.background = ''
    drawContents()
    dom.search.focus()
  }

  function closeThread() {
    stopReveal(true)
    if (state.unsubThread) {
      try {
        state.unsubThread()
      } catch (_e) {
        // The frame is going away; nothing to undo.
      }
    }
    state.unsubThread = null
    state.view = null
    state.older = []
    state.hasMoreOlder = false
    state.page = -1
    state.seenSeq = -1
    state.absorbedIds = {}
  }

  function openAgent(address) {
    closeThread()
    state.open = address
    dom.contents.hidden = true
    dom.leaf.hidden = false
    turn()
    drawLeaf()
    // A Conversation widget beside the Diary may follow it.
    if (can('conversation.open')) k2.conversation.open(address).catch(function () {})
    if (!can('thread.subscribe')) {
      state.view = { address: address, phase: 'unavailable', note: 'Allow this Diary to read your conversations to see its pages.', items: [], hasMore: false }
      drawLeaf()
      return
    }
    var first = true
    state.unsubThread = k2.thread.subscribe(address, function (view) {
      if (state.open !== address) return
      state.view = view
      if (first && view.phase === 'ready') {
        // Everything already on the page is history: no handwriting.
        first = false
        var items = view.items || []
        if (items.length) state.seenSeq = items[items.length - 1].seq
        state.hasMoreOlder = !!view.hasMore
      }
      drawLeaf({ reveal: true })
    })
    if (state.page < 0) dom.ink.focus()
  }

  function loadOlder() {
    if (state.loadingOlder || !state.open) return
    var items = allItems()
    if (!items.length) return
    state.loadingOlder = true
    var address = state.open
    k2.thread
      .read(address, { beforeSeq: items[0].seq, limit: OLDER_PAGE })
      .then(function (page) {
        if (state.open !== address) return
        var before = exchanges().length
        state.older = (page.items || []).concat(state.older)
        state.hasMoreOlder = !!page.hasMore
        var added = exchanges().length - before
        state.page = Math.max(0, added - 1)
        turn('back')
        drawLeaf()
      })
      .catch(function (e) {
        dom.note.hidden = false
        showError(dom.note, e)
      })
      .then(function () {
        state.loadingOlder = false
      })
  }

  function prevPage() {
    var list = exchanges()
    var idx = pageIndex(list)
    if (idx > 0) {
      state.page = idx - 1
      turn('back')
      drawLeaf()
    } else if (state.hasMoreOlder || (state.view && state.view.hasMore)) {
      loadOlder()
    }
  }

  function nextPage() {
    var list = exchanges()
    var idx = pageIndex(list)
    if (idx < list.length - 1) {
      state.page = idx + 1 === list.length - 1 ? -1 : idx + 1
      turn()
      drawLeaf()
    }
  }

  // ── writing: the ink soaks into the page ─────────────────────────────

  function send() {
    var words = dom.ink.value.trim()
    if (!words || state.sending || !state.open) return
    state.sending = true
    dom.send.disabled = true
    dom.soak.textContent = dom.ink.value
    dom.ink.value = ''
    dom.soak.classList.remove('on')
    if (!reduced()) {
      void dom.soak.offsetWidth
      dom.soak.classList.add('on')
    } else {
      dom.soak.textContent = ''
    }
    var address = state.open
    k2.thread
      .post(address, words)
      .then(function (posted) {
        if (posted && posted.id) state.absorbedIds[posted.id] = true
        state.page = -1
        if (state.open === address) drawLeaf()
      })
      .catch(function (e) {
        if (state.open !== address) return
        // Give the words back: nothing was sent.
        dom.ink.value = words
        dom.soak.textContent = ''
        dom.note.hidden = false
        showError(dom.note, e)
      })
      .then(function () {
        state.sending = false
        dom.send.disabled = false
      })
  }

  // ── wiring ───────────────────────────────────────────────────────────

  function applyTheme(info) {
    if (!info) return
    var theme = info.theme || {}
    if (theme.scheme) dom.root.setAttribute('data-zen-scheme', theme.scheme)
    var vars = theme.vars || {}
    Object.keys(vars).forEach(function (k) {
      if (k.indexOf('--zen-') === 0) dom.root.style.setProperty(k, vars[k])
    })
    state.reduced = !!(info.motion && info.motion.reduced)
    dom.root.classList.toggle('reduced', state.reduced)
  }

  function ready() {
    if (state.readyDone) return
    state.readyDone = true
    if (k2 && typeof k2.ready === 'function') k2.ready()
  }

  function start() {
    if (!k2) {
      dom.tocEmpty.hidden = false
      text(dom.tocEmpty, 'This page only works inside a K2 Garden.')
      return
    }
    var cfg = k2.config || {}
    if (typeof cfg.title === 'string' && cfg.title.trim()) text(dom.title, cfg.title.trim().slice(0, 40))
    state.reduced = !!(k2.motion && k2.motion.reduced)
    dom.root.classList.toggle('reduced', state.reduced)
    if (can('theme.get')) k2.theme.get().then(applyTheme).catch(function () {})
    if (can('theme.changed')) k2.theme.changed(applyTheme)

    dom.search.addEventListener('input', function () {
      state.query = dom.search.value
      drawContents()
    })
    dom.search.addEventListener('keydown', function (e) {
      if (e.key !== 'Enter') return
      var rows = filtered().filter(function (r) { return r.openable })
      if (rows.length) openAgent(rows[0].address)
    })
    dom.back.addEventListener('click', showContents)
    dom.prev.addEventListener('click', prevPage)
    dom.next.addEventListener('click', nextPage)
    dom.pen.addEventListener('submit', function (e) {
      e.preventDefault()
      send()
    })
    dom.ink.addEventListener('keydown', function (e) {
      if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
        e.preventDefault()
        send()
      }
    })
    dom.soak.addEventListener('animationend', function () {
      dom.soak.classList.remove('on')
      dom.soak.textContent = ''
    })
    dom.sheet.addEventListener('click', function (e) {
      // A tap anywhere on the page shows the whole reply.
      if (state.revealing && !dom.pen.contains(e.target)) stopReveal(true)
    })
    document.addEventListener('keydown', function (e) {
      if (dom.leaf.hidden || e.target === dom.ink || e.target === dom.search) return
      if (e.key === 'ArrowLeft') prevPage()
      else if (e.key === 'ArrowRight') nextPage()
      else if (e.key === 'Escape') showContents()
    })

    if (!can('agents.subscribe')) {
      dom.tocEmpty.hidden = false
      text(dom.tocEmpty, 'Allow this Diary to see your agents to begin.')
      ready()
      return
    }
    k2.agents.subscribe(function (rows) {
      state.rows = (rows || []).slice().sort(function (a, b) { return a.index - b.index })
      if (state.open) {
        if (!rowFor(state.open)) return showContents()
        drawLeaf()
      } else {
        drawContents()
        // One agent in scope: open its page straight away.
        if (state.rows.length === 1 && state.rows[0].openable && !state.query) openAgent(state.rows[0].address)
      }
      ready()
    })
  }

  start()
})()
