// The Workbench's inbox (k2:work-inbox@1, 0.46.7). Your agents' tasks,
// sorted by what needs you:
//   tasks      k2.work.list(): every sidecar here, its worktree, branch,
//              ahead / uncommitted (the daemon's truth)
//   needs you  k2.thread.unread(): cards waiting in a task's Thread; and
//              k2.thread.onActivity(): a turn that needs you
//   working    k2.thread.onActivity(): the Thread turn
//   ready      in its own worktree, idle, with commits or changes
//   marks      pin, snooze-until and hide live in this widget's data
//              (`item/<ws>/<name>`), so they follow you to every window
//              and an agent you allow can read them (k2 store --widget
//              k2:work-inbox); a snoozed task comes back at its time
//   pick       `sel` in this widget's data (the Review widget reads it),
//              k2.agents.show + k2.conversation.open (the Thread column)
//   other      agents on your other computers (your Homes' rows), read
//              only: a Thread runs where its agent runs
// Every write (pin, start, stop) runs only from your click in this widget.
(function () {
  'use strict'
  var k2 = window.k2
  var WB = window.WB
  var el = WB.el
  var listEl = document.getElementById('list')
  var bannerEl = document.getElementById('banner')
  var filterEl = document.getElementById('filter')
  var hiddenBtn = document.getElementById('btnHidden')
  var newBtn = document.getElementById('btnNew')

  var tasks = []
  var rows = []
  var waiting = {}
  var unread = {}
  var turns = {}
  var marks = {}
  var sel = null
  var showHidden = false
  var folded = { snoozed: true, settled: true, hidden: false, other: false }
  var activity = {}
  var timer = null

  function can(verb) {
    return !!(k2 && typeof k2.can === 'function' && k2.can(verb))
  }

  function banner(text) {
    bannerEl.hidden = !text
    bannerEl.textContent = text || ''
  }

  function threadOf(address) {
    return String(address || '').split('::')[0]
  }

  // ── Data ──────────────────────────────────────────────────────────────
  async function loadTasks() {
    if (!can('work.list')) return
    try {
      tasks = (await k2.work.list()) || []
    } catch (e) {
      banner('Tasks: ' + WB.errText(e))
    }
  }

  async function loadRows() {
    try {
      rows = (await k2.agents.list()) || []
    } catch (e) {
      rows = []
    }
  }

  async function loadUnread() {
    if (!can('thread.unread')) return
    try {
      var u = await k2.thread.unread()
      var w = {}
      var n = {}
      ;(u && u.byConversation ? u.byConversation : []).forEach(function (r) {
        if (!r.addr) return
        var a = String(r.addr).toLowerCase()
        w[a] = (w[a] || 0) + (r.waiting || 0)
        n[a] = (n[a] || 0) + (r.muted ? 0 : r.unread || 0)
      })
      waiting = w
      unread = n
    } catch (e) {
      // Counts are a nicety: the list still works without them.
    }
  }

  // One Thread-turn feed per task (≤ 12: K2 allows 16 Thread feeds).
  function syncActivity() {
    if (!can('thread.onActivity')) return
    var want = tasks.slice(0, 12).map(function (t) { return t.address })
    Object.keys(activity).forEach(function (a) {
      if (want.indexOf(a) < 0) {
        try { activity[a]() } catch (e) { /* already ended */ }
        delete activity[a]
        delete turns[a]
      }
    })
    want.forEach(function (a) {
      if (activity[a]) return
      activity[a] = k2.thread.onActivity(
        a,
        function (v) {
          turns[a] = v && v.live && v.turn ? v.turn.state : null
          render()
        },
        function () {
          delete activity[a]
        }
      )
    })
  }

  async function loadMarks() {
    var next = {}
    var after
    do {
      var page = await k2.store.list({ prefix: 'item/', after: after, limit: 500 })
      ;(page && page.items ? page.items : []).forEach(function (it) { next[it.key] = it.value || {} })
      after = page && page.next ? page.next : undefined
    } while (after)
    marks = next
  }

  function onMarks(changes) {
    var list = Array.isArray(changes) ? changes : changes && changes.items ? changes.items : []
    if (changes && changes.resync) marks = {}
    list.forEach(function (c) {
      if (c.value === null || c.value === undefined) delete marks[c.key]
      else marks[c.key] = c.value
    })
    render()
  }

  async function refresh() {
    await Promise.all([loadTasks(), loadRows(), loadUnread()])
    syncActivity()
    render()
  }

  // ── Marks (this widget's data) ────────────────────────────────────────
  function markOf(thread) {
    return marks[WB.itemKey(thread)] || {}
  }

  async function setMark(thread, patch) {
    var key = WB.itemKey(thread)
    var next = Object.assign({}, markOf(thread), patch, { updatedAt: Date.now() })
    Object.keys(next).forEach(function (k) { if (next[k] === null || next[k] === false) delete next[k] })
    marks[key] = next
    render()
    try {
      await k2.store.put(key, next)
    } catch (e) {
      banner('Couldn’t save that: ' + WB.errText(e))
    }
  }

  // ── Picking a task ────────────────────────────────────────────────────
  async function pick(address, task) {
    sel = address
    render()
    var thread = threadOf(address)
    var m = markOf(thread)
    if (m.snoozeUntil && m.snoozeUntil <= Date.now()) setMark(thread, { snoozeUntil: null, woke: null })
    try {
      if (can('agents.show')) await k2.agents.show([address])
      await k2.conversation.open(address)
    } catch (e) {
      banner('Couldn’t show ' + thread + ': ' + WB.errText(e))
    }
    var t = task || {}
    try {
      await k2.store.put('sel', {
        address: address,
        thread: thread,
        name: t.name || thread,
        workspace: t.workspace || thread.split('/')[0],
        worktree: t.worktree || null,
        branch: t.branch || null,
        base: t.base || null,
        remote: WB.remote(address),
        at: Date.now(),
      })
    } catch (e) {
      // The Review column just won't follow.
    }
  }

  // ── Drawing ───────────────────────────────────────────────────────────
  var ORDER = [
    ['pinned', 'Pinned'],
    ['needs-you', 'Needs you'],
    ['working', 'Working'],
    ['ready', 'Ready to land'],
    ['idle', 'Idle'],
    ['snoozed', 'Snoozed'],
    ['settled', 'Settled'],
    ['hidden', 'Hidden'],
    ['other', 'Other computers (read only)'],
  ]

  function matches(text) {
    var f = filterEl.value.trim().toLowerCase()
    return !f || String(text).toLowerCase().indexOf(f) >= 0
  }

  function factsOf(t) {
    var th = threadOf(t.address)
    return { waiting: waiting[th] || 0, turn: turns[t.address] || null }
  }

  function closeMenu() {
    var m = document.querySelector('.menu')
    if (m) m.remove()
  }

  function snoozeMenu(btn, thread) {
    closeMenu()
    var r = btn.getBoundingClientRect()
    var menu = el('div', 'menu')
    menu.style.left = Math.min(r.left, window.innerWidth - 180) + 'px'
    menu.style.top = r.bottom + 4 + 'px'
    WB.snoozePresets().forEach(function (p) {
      var b = el('button', '', p.label)
      b.type = 'button'
      b.addEventListener('click', function (ev) {
        ev.stopPropagation()
        closeMenu()
        setMark(thread, { snoozeUntil: p.at, hidden: null })
      })
      menu.appendChild(b)
    })
    document.body.appendChild(menu)
  }

  function actionsFor(t, thread, m, isRemote) {
    var acts = el('div', 'acts')
    function add(label, fn, cls, title) {
      var b = el('button', 'btn small' + (cls ? ' ' + cls : ''), label)
      b.type = 'button'
      if (title) b.title = title
      b.addEventListener('click', function (ev) {
        ev.stopPropagation()
        fn(b)
      })
      acts.appendChild(b)
    }
    add(m.pinned ? 'Unpin' : 'Pin', function () { setMark(thread, { pinned: !m.pinned }) })
    if (m.snoozeUntil && m.snoozeUntil > Date.now()) add('Wake now', function () { setMark(thread, { snoozeUntil: null }) })
    else add('Snooze…', function (b) { snoozeMenu(b, thread) })
    add(m.hidden ? 'Unhide' : 'Hide', function () { setMark(thread, { hidden: !m.hidden }) })
    if (!isRemote && t && t.live && can('agents.stop') && WB.isSidecar(t.address)) {
      add('Stop', async function () {
        try {
          await k2.agents.stop({ address: t.address })
        } catch (e) {
          banner('Stop: ' + WB.errText(e))
        }
      }, 'danger', 'Stop its current turn')
    }
    if (!isRemote && can('agents.openTerminal')) {
      add('Terminal', async function () {
        try {
          await k2.agents.openTerminal(t ? t.address : thread + '::local')
        } catch (e) {
          banner('Terminal: ' + WB.errText(e))
        }
      })
    }
    return acts
  }

  function taskRow(t, state, m) {
    var thread = threadOf(t.address)
    var row = el('div', 'row' + (sel === t.address ? ' selected' : ''))
    row.tabIndex = 0
    row.setAttribute('role', 'button')
    var cls = state === 'needs-you' ? 'needs' : state === 'working' ? 'working' : state === 'ready' ? 'ready' : ''
    row.appendChild(el('span', 'dot ' + cls))
    row.appendChild(el('span', 'name', t.name || thread))
    var woke = m.snoozeUntil && m.snoozeUntil <= Date.now()
    var label = woke ? 'Woke' : m.snoozeUntil ? 'until ' + WB.until(m.snoozeUntil) : WB.LABEL[state] || state
    row.appendChild(el('span', 'label ' + cls, label))
    var sub = el('div', 'sub')
    sub.appendChild(el('span', '', t.workspaceLabel || t.workspace || ''))
    if (t.branch) sub.appendChild(el('span', 'mono', t.branch))
    if (t.ahead) sub.appendChild(el('span', 'chip', t.ahead + ' ahead'))
    if (t.dirty) sub.appendChild(el('span', 'chip', t.dirty + ' changed'))
    if (!t.worktree) sub.appendChild(el('span', 'chip', 'no worktree'))
    var n = unread[thread] || 0
    if (n) sub.appendChild(el('span', 'chip unread', n + ' new'))
    if (t.harness) sub.appendChild(el('span', '', t.harness))
    row.appendChild(sub)
    row.appendChild(actionsFor(t, thread, m, false))
    row.addEventListener('click', function () { pick(t.address, t) })
    row.addEventListener('keydown', function (ev) {
      if (ev.key === 'Enter' && ev.target === row) pick(t.address, t)
    })
    return row
  }

  function remoteRow(r, m) {
    var row = el('div', 'row' + (sel === r.address ? ' selected' : ''))
    row.tabIndex = 0
    var cls = r.needsYou ? 'needs' : r.working ? 'working' : ''
    row.appendChild(el('span', 'dot ' + cls))
    row.appendChild(el('span', 'name', r.label || r.address))
    row.appendChild(el('span', 'label ' + cls, r.needsYou ? 'Needs you' : r.working ? 'Working' : r.server || ''))
    var sub = el('div', 'sub')
    sub.appendChild(el('span', '', r.server || r.address.split('::')[1]))
    row.appendChild(sub)
    row.appendChild(actionsFor(null, threadOf(r.address) + '@' + r.address.split('::')[1], m, true))
    row.addEventListener('click', function () { pick(r.address, { name: r.label }) })
    return row
  }

  function render() {
    var now = Date.now()
    var groups = {}
    ORDER.forEach(function (o) { groups[o[0]] = [] })
    tasks.forEach(function (t) {
      if (!matches((t.name || '') + ' ' + (t.workspaceLabel || '') + ' ' + (t.branch || ''))) return
      var m = markOf(threadOf(t.address))
      var state = WB.taskState(t, factsOf(t))
      var s = WB.section(state, m, now)
      groups[s].push(taskRow(t, state, m))
    })
    rows.forEach(function (r) {
      if (!WB.remote(r.address) || !matches(r.label || r.address)) return
      var m = markOf(threadOf(r.address) + '@' + r.address.split('::')[1])
      if (m.hidden && !showHidden) return
      groups.other.push(remoteRow(r, m))
    })
    listEl.textContent = ''
    var any = false
    ORDER.forEach(function (o) {
      var id = o[0]
      var items = groups[id]
      if (!items.length || (id === 'hidden' && !showHidden)) return
      any = true
      var sec = el('section', 'section')
      var h = el('h4', '', o[1] + ' (' + items.length + ')')
      if (id === 'snoozed' || id === 'settled' || id === 'other') {
        var tg = el('button', 'btn small', folded[id] ? 'Show' : 'Fold')
        tg.type = 'button'
        tg.addEventListener('click', function () {
          folded[id] = !folded[id]
          render()
        })
        h.appendChild(tg)
      }
      sec.appendChild(h)
      if (!folded[id]) items.forEach(function (r) { sec.appendChild(r) })
      listEl.appendChild(sec)
    })
    if (!any) {
      listEl.appendChild(
        el('p', 'empty', can('work.list')
          ? 'No tasks yet. Press “+ New task” to start an agent in its own worktree, or ask an agent to open a sidecar (k2 sidecar new).'
          : 'This widget can’t list tasks here.')
      )
    }
    hiddenBtn.setAttribute('aria-pressed', showHidden ? 'true' : 'false')
    newBtn.hidden = !can('agents.start')
    wakeLater(now)
  }

  // A snoozed task comes back at its time: redraw then.
  function wakeLater(now) {
    if (timer) clearTimeout(timer)
    timer = null
    var next = Infinity
    Object.keys(marks).forEach(function (k) {
      var at = marks[k] && marks[k].snoozeUntil
      if (at && at > now && at < next) next = at
    })
    if (next !== Infinity) timer = setTimeout(render, Math.min(next - now + 500, 2147483000))
  }

  // ── New task ──────────────────────────────────────────────────────────
  var dlg = document.getElementById('newTask')
  var form = document.getElementById('newForm')
  var wsSel = document.getElementById('ntWs')
  var note = document.getElementById('ntNote')

  function openNew() {
    wsSel.textContent = ''
    rows
      .filter(function (r) { return !WB.isSidecar(r.address) && !WB.remote(r.address) })
      .forEach(function (r) {
        var o = el('option', '', r.label || r.address)
        o.value = r.address
        wsSel.appendChild(o)
      })
    if (sel && !WB.remote(sel)) {
      var parent = threadOf(sel).split('/')[0] + '::local'
      if (Array.prototype.some.call(wsSel.options, function (o) { return o.value === parent })) wsSel.value = parent
    }
    note.textContent = 'K2 asks you to confirm, then starts the agent in .worktrees/k2-<name> on branch k2/<name>. Agents run on this computer.'
    dlg.hidden = false
    document.getElementById('ntName').focus()
  }

  form.addEventListener('submit', async function (ev) {
    ev.preventDefault()
    var args = {
      workspace: wsSel.value,
      name: document.getElementById('ntName').value.trim(),
      brief: document.getElementById('ntBrief').value,
    }
    var base = document.getElementById('ntBase').value.trim()
    var harness = document.getElementById('ntHarness').value.trim()
    if (base) args.base = base
    if (harness) args.harness = harness
    if (!args.workspace || !args.name || !args.brief.trim()) {
      note.textContent = 'Pick a workspace, and give it a name and a brief.'
      return
    }
    var go = document.getElementById('ntGo')
    go.disabled = true
    note.textContent = 'Asking K2…'
    try {
      var t = await k2.agents.start(args)
      dlg.hidden = true
      form.reset()
      await refresh()
      if (t && t.address) pick(t.address, t)
    } catch (e) {
      note.textContent = e && e.code === 'declined' ? 'Not started.' : WB.errText(e)
    } finally {
      go.disabled = false
    }
  })
  document.getElementById('ntCancel').addEventListener('click', function () { dlg.hidden = true })
  dlg.addEventListener('keydown', function (ev) { if (ev.key === 'Escape') dlg.hidden = true })
  newBtn.addEventListener('click', openNew)
  hiddenBtn.addEventListener('click', function () {
    showHidden = !showHidden
    render()
  })
  filterEl.addEventListener('input', render)
  document.addEventListener('click', function (ev) {
    if (!(ev.target && ev.target.closest && ev.target.closest('.menu'))) closeMenu()
  })

  // ── Start ─────────────────────────────────────────────────────────────
  async function start() {
    try {
      var s = await k2.store.get('sel')
      if (s && s.value && s.value.address) sel = s.value.address
    } catch (e) {
      // No pick yet.
    }
    try {
      await loadMarks()
      k2.store.subscribe('item/', onMarks, function (e) { banner('Live marks stopped: ' + WB.errText(e)) })
    } catch (e) {
      banner('Pins and snoozes: ' + WB.errText(e))
    }
    await refresh()
    if (sel && can('agents.show')) k2.agents.show([sel]).catch(function () {})
    k2.ready()
    setInterval(function () {
      if (document.visibilityState !== 'hidden') refresh()
    }, 10000)
  }
  start()
})()
