// The Thread beside the board (k2:kanban@5). It is K2's own Conversation
// widget, not a copy: the Garden file puts it in a collapsible column
// labelled "Thread" with agents = "<this widget's id>" and pick = "all", so
// its header is the drop-down that picks any of your agents, and secret and
// permission cards, files, mute, earlier messages and the activity drawer
// are K2's. The header's pick and this widget share one selection: the row
// k2.agents.subscribe marks `selected` is the agent the header shows, and
// k2.conversation.open shows another one there.
//   Thread       folds and unfolds that column (k2.layout.toggle, after
//                your click), pressed while it is open (k2.layout.subscribe)
//   Send board   posts the board, as markdown, to the agent the header shows
//                (k2.thread.post)
//   look         [widget.props.config] look = "garden": the Garden theme's
//                colours instead of K2 Companion's
//   carry-over   a board whose @4 panel had an agent picked (k2.save
//                "dock/<garden>") shows that agent once, when the Thread
//                shows no one yet; then the old save is removed
// Agent text never reaches this file: the board's own text goes out.
(function () {
  'use strict'
  var k2 = window.k2
  var board = window.KanbanBoard
  var toggle = document.getElementById('btnThread')
  var sendBtn = document.getElementById('btnSend')

  var column = -1
  var shown = null
  var carry = null
  var sending = false

  function errText(e) {
    if (!e) return 'error'
    if (e.code === 'sending_off') return 'posting is paused; click Resume on the widget'
    return e.code || e.message || 'error'
  }

  // The Thread's column: the collapsible one labelled "Thread", else the
  // first collapsible column. -1: this page has none (no Thread button).
  function threadColumn(columns) {
    var cols = Array.isArray(columns) ? columns : []
    var named = cols.find(function (c) { return c && c.collapsible && c.label === 'Thread' })
    var any = named || cols.find(function (c) { return c && c.collapsible })
    return any && typeof any.index === 'number' ? any : null
  }

  function drawLayout(info) {
    var col = threadColumn(info && info.columns)
    column = col ? col.index : -1
    toggle.hidden = column < 0
    toggle.setAttribute('aria-pressed', col && !col.collapsed ? 'true' : 'false')
    if (board) board.shareToolbar()
  }

  function paintSend() {
    var can = !!(k2 && typeof k2.can === 'function' && k2.can('thread.post'))
    sendBtn.hidden = !can
    sendBtn.disabled = !shown || sending
    sendBtn.title = shown
      ? 'Post the board, as markdown, to ' + (shown.label || shown.address) + ' in the Thread'
      : 'Pick an agent in the Thread first'
    if (board) board.shareToolbar()
  }

  // ── The agent the Thread shows ────────────────────────────────────────────
  function onRows(list) {
    var rows = Array.isArray(list) ? list.filter(function (r) { return r && typeof r.address === 'string' && r.address }) : []
    var sel = rows.find(function (r) { return r.selected }) || null
    shown = sel ? { address: sel.address, label: sel.label || sel.address } : null
    paintSend()
    carryOver(rows)
  }

  function saveKey() {
    var g = board && board.garden ? board.garden() : ''
    return 'dock/' + (g.replace(/[^A-Za-z0-9._-]/g, '') || 'main')
  }

  // @4 kept the panel's pick in this widget's save. Show it once, only when
  // the Thread shows no one, and only if that agent is in reach; then drop
  // the old save so it never overrides a pick made in the header.
  function carryOver(rows) {
    if (!carry || shown) return
    var want = carry
    if (!rows.some(function (r) { return r.address === want })) return
    carry = null
    k2.conversation.open(want).catch(function () {
      // Not shown: the person picks in the Thread's header.
    })
    k2.save.remove(saveKey()).catch(function () {
      // Kept: it only applies while the Thread shows no one.
    })
  }

  function loadCarry() {
    if (!k2.save || !k2.can('save.get') || !k2.can('save.remove') || !k2.can('conversation.open')) return Promise.resolve()
    return k2.save.get(saveKey()).then(function (saved) {
      var s = saved && typeof saved === 'object' ? saved : {}
      if (typeof s.agent === 'string' && s.agent.indexOf('::') > 0) carry = s.agent
    }, function () {
      // Nothing to carry.
    })
  }

  // ── Buttons ───────────────────────────────────────────────────────────────
  toggle.addEventListener('click', function () {
    if (column < 0) return
    k2.layout.toggle(column).then(function (col) {
      toggle.setAttribute('aria-pressed', col && !col.collapsed ? 'true' : 'false')
    }, function (e) {
      if (board) board.toast('Thread: ' + errText(e), true)
    })
  })

  sendBtn.addEventListener('click', function () {
    if (!shown || !board || sending) return
    var to = shown
    sending = true
    paintSend()
    k2.thread.post(to.address, board.exportMarkdown()).then(function () {
      sending = false
      paintSend()
      board.toast('Sent the board to ' + to.label)
    }, function (e) {
      sending = false
      paintSend()
      board.toast('Send failed: ' + errText(e), true)
    })
  })

  // For tests.
  window.KanbanThread = {
    look: function () { return document.documentElement.classList.contains('look-garden') ? 'garden' : 'companion' },
    column: function () { return column },
    shown: function () { return shown ? shown.address : '' }
  }

  if (!k2) return
  k2.connected.then(function () {
    // K2 Companion's look is the default (kanban.css); a Garden file can
    // ask for the Garden theme's colours instead.
    if (k2.config && k2.config.look === 'garden') document.documentElement.classList.add('look-garden')
    if (k2.can('layout.get')) {
      k2.layout.get().then(drawLayout, function () { drawLayout(null) })
      if (k2.can('layout.subscribe')) k2.layout.subscribe(drawLayout)
    }
    if (!k2.can('agents.subscribe')) return
    return loadCarry().then(function () {
      k2.agents.subscribe(onRows, function () {
        shown = null
        paintSend()
      })
    })
  }).catch(function () {
    // The board says when K2 didn't start the widget.
  }).then(function () {
    if (board) board.shareToolbar()
  })
})()
