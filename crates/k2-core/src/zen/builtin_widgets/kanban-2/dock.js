// The Thread panel beside the board (k2:kanban@2):
// talk to one of your agents next to the board, and Send board posts the
// board as markdown. It wears K2's Thread look (components/Thread/
// ThreadZenKit: ThreadBubble, ThreadComposeBox, ThreadSendButton, and
// Zen/widgets/ZenActivityDrawer's ear) in K2 Companion's colours
// (kanban.css): rounded bubbles, yours on the right and the agent's on the
// left, "author · time" under each, markdown inside; a rounded field with a
// round Send (Enter sends, Shift+Enter is a new line); and the working ear
// on the field's top edge (dot, state, counters, clock) while the agent
// works. A sealed widget can't load those React pieces, so this redraws
// them with the same rules. Optional: closed until you press Thread, and no agent
// is picked until you pick one (or the Garden file names one in
// [widget.props.config] agent = "<handle>::<host>"). Whether it's open and
// who you picked are this widget's private saves (k2.save, per Garden), so
// they come back next time; without saves they last until a reload.
//   agents    k2.agents.subscribe   (only once the panel is opened)
//   history   k2.thread.subscribe   (live)
//   post      k2.thread.post
//   choices   k2.thread.answer
// Secret cards are answered in K2's own Thread (widgets can't send
// secrets). Agent text goes in with textContent or md.js's fragment.
(function () {
  'use strict'
  var k2 = window.k2
  var board = window.KanbanBoard
  var dock = document.getElementById('threadDock')
  var toggle = document.getElementById('btnThread')
  var bodyEl = document.getElementById('dockBody')
  var turnEl = document.getElementById('dockTurn')
  var postForm = document.getElementById('dockPost')
  var textEl = document.getElementById('dockText')
  var fallbackEl = document.getElementById('dockFallback')
  var sendBtn = document.getElementById('btnSend')
  var sendRound = document.getElementById('dockSendBtn')

  var current = { address: '', label: '' }
  var offThread = null
  var rows = []
  var chosen = ''
  var seenLabels = {}
  var picker = null
  var started = false
  var open = false
  var posting = false

  function el(tag, cls, text) {
    var n = document.createElement(tag)
    if (cls) n.className = cls
    if (text != null) n.textContent = text
    return n
  }

  function fillMarkdown(node, text) {
    if (window.KanbanMarkdown) window.KanbanMarkdown.fill(node, text)
    else node.textContent = String(text || '')
  }

  function note(text, cls) {
    bodyEl.replaceChildren(el('div', cls || 'setup', text))
  }

  function dockErr(msg) {
    var e = bodyEl.querySelector('.dock-err')
    if (!e) {
      e = el('div', 'dock-err')
      bodyEl.insertBefore(e, bodyEl.firstChild)
    }
    e.textContent = msg
  }

  function errText(e) {
    if (!e) return 'error'
    if (e.code === 'sending_off') return 'posting is paused; click Resume on the widget'
    return e.code || e.message || 'error'
  }

  // ── Saved state (k2.save, private to this widget, one key per Garden) ─────
  function saveKey() {
    var g = board && board.garden ? board.garden() : ''
    return 'dock/' + (g.replace(/[^A-Za-z0-9._-]/g, '') || 'main')
  }

  function canSave() {
    return !!(k2 && k2.save && typeof k2.can === 'function' && k2.can('save.set'))
  }

  function remember() {
    if (!canSave()) return
    k2.save.set(saveKey(), { open: open, agent: chosen || null }).catch(function () {
      // Not saved: it lasts until a reload.
    })
  }

  // ── Messages ──────────────────────────────────────────────────────────────
  // A person's post (yours, from K2's Thread or this panel) says
  // via=compose; everything else is the agent's.
  function isMine(doc) {
    return doc.via === 'compose'
  }

  // "You"; the agent's name from the agent list, else its address.
  function authorOf(doc, mine) {
    if (mine) return 'You'
    var from = String(doc.from || '')
    if (!from) return 'agent'
    var row = rows.find(function (r) { return r.address === from })
    if (row && row.label) return row.label
    var here = current.address
    if (here && (from === here || from === here.split('::')[0])) return current.label || from
    return from
  }

  // K2's relative time (lib/format-relative-time): just now, 5m ago, 2h ago.
  function ago(sec, now) {
    var d = Math.max(0, now - sec)
    if (d < 45) return 'just now'
    if (d < 90) return '1m ago'
    var mins = Math.round(d / 60)
    if (mins < 60) return mins + 'm ago'
    var hours = Math.round(d / 3600)
    if (hours < 24) return hours + 'h ago'
    return Math.round(d / 86400) + 'd ago'
  }

  function nowSec() {
    return Math.floor(Date.now() / 1000)
  }

  function choiceRow(id, c) {
    var opts = el('div', 'choice-opts')
    ;(c.options || []).forEach(function (opt) {
      var text = opt == null ? '' : typeof opt === 'string' ? opt : String(opt.label || opt.value || '')
      if (!text) return
      var b = el('button', 'chip', text)
      b.type = 'button'
      b.addEventListener('click', function () { answerChoice(id, text) })
      opts.appendChild(b)
    })
    return opts
  }

  function renderItem(item, now) {
    var doc = (item && item.doc) || {}
    var id = item.id || doc.id || ''
    var kind = doc.kind || 'text'
    var mine = isMine(doc)
    var wrap = el('div', 'msg ' + (mine ? 'mine' : 'theirs'))
    wrap.setAttribute('data-kind', kind)
    if (id) wrap.setAttribute('data-msg-id', id)
    if (typeof item.seq === 'number') wrap.setAttribute('data-seq', String(item.seq))
    var bubble = el('div', 'bubble ' + (mine ? 'bubble-me' : 'bubble-agent'))
    wrap.appendChild(bubble)

    if (kind === 'choice') {
      var c = doc.choice || {}
      var prompt = el('div', 'body')
      fillMarkdown(prompt, c.prompt || doc.body || '')
      bubble.appendChild(prompt)
      if (c.status === 'pending') {
        bubble.appendChild(choiceRow(id, c))
        if (c.allow_custom) {
          var row = el('div', 'secret-row')
          var inp = el('input')
          inp.type = 'text'
          inp.placeholder = 'Custom answer'
          var go = el('button', 'chip', 'Answer')
          go.type = 'button'
          go.addEventListener('click', function () {
            var t = (inp.value || '').trim()
            if (t) answerChoice(id, t)
          })
          row.appendChild(inp)
          row.appendChild(go)
          bubble.appendChild(row)
        }
      } else {
        bubble.appendChild(el('div', 'status', c.status === 'voided' ? 'Voided' : 'Answered' + (c.answer ? ': ' + c.answer : '')))
      }
    } else if (kind === 'secret') {
      bubble.appendChild(el('div', 'body', doc.body || 'Secret requested'))
      bubble.appendChild(el('div', 'status', 'Answer secret cards in K2’s own Thread (widgets can’t send secrets).'))
    } else {
      var body = el('div', 'body')
      fillMarkdown(body, doc.body || '')
      bubble.appendChild(body)
    }

    // "author · time" under the bubble, on its side.
    var meta = el('div', 'meta')
    meta.appendChild(el('span', 'author', authorOf(doc, mine)))
    if (typeof doc.created_at === 'number' && isFinite(doc.created_at)) {
      meta.appendChild(document.createTextNode(' · '))
      var t = el('span', 'time', ago(doc.created_at, now))
      t.setAttribute('data-at', String(doc.created_at))
      meta.appendChild(t)
    }
    wrap.appendChild(meta)
    return wrap
  }

  // The times say "5m ago"; they move on without a new message.
  function refreshTimes() {
    var now = nowSec()
    bodyEl.querySelectorAll('.time[data-at]').forEach(function (t) {
      var at = Number(t.getAttribute('data-at'))
      if (isFinite(at)) t.textContent = ago(at, now)
    })
  }
  setInterval(refreshTimes, 30000)

  function drawView(view) {
    if (!view || view.address !== current.address) return
    var atBottom = bodyEl.scrollHeight - bodyEl.scrollTop - bodyEl.clientHeight < 40
    if (view.phase === 'unavailable' || view.phase === 'failed') {
      note(view.note || 'This Thread isn’t available right now.', 'setup')
      return
    }
    var now = nowSec()
    var items = view.items || []
    if (items.length) {
      var list = el('div', 'msgs')
      items.forEach(function (item) { list.appendChild(renderItem(item, now)) })
      bodyEl.replaceChildren(list)
    } else {
      note(view.phase === 'opening' ? 'Opening…' : 'Message ' + (current.label || 'this agent') + '. It reads this like a text.', 'dock-note dock-empty')
    }
    lastView = view
    noteReplies(view)
    paintStrip()
    if (atBottom) bodyEl.scrollTop = bodyEl.scrollHeight
  }

  // ── Working ear ───────────────────────────────────────────────────────────
  // After K2's activity drawer (ZenActivityDrawer, from K2 Companion): a tab
  // pulled out of the message field's top edge while the agent works. One
  // line: a dot in the state's colour (pulsing while it works), the state,
  // tinted counters (subagents, tool calls, commands) and the turn's clock.
  // Live only; driven by the open Thread's `turn` and the picked agent's
  // row (`counts`). After a post it says "Sent" until the agent reports
  // working or a reply lands.
  var PENDING_MAX_MS = 3 * 60 * 1000
  var lastView = null
  var pending = null // { since, seq }
  var localSince = { tone: '', at: 0 }
  var sawWorking = false
  var tick = null
  var SVG = 'http://www.w3.org/2000/svg'
  // 24×24 strokes (K2's drawer icons): a robot, a wrench, a prompt.
  var ICONS = {
    subagents: ['M6 7h12a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2z', 'M12 3v4', 'M9 13h.01M15 13h.01', 'M2 12v3M22 12v3'],
    tools: ['M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.8-3.8a6 6 0 0 1-7.9 7.9l-6.9 6.9a2.1 2.1 0 0 1-3-3l6.9-6.9a6 6 0 0 1 7.9-7.9z'],
    commands: ['M4 17l6-6-6-6', 'M12 19h8']
  }
  var COUNTS = [['subagents', 'subagent', 'subagents'], ['tools', 'tool call', 'tool calls'], ['commands', 'command', 'commands']]

  function icon(name) {
    var svg = document.createElementNS(SVG, 'svg')
    svg.setAttribute('viewBox', '0 0 24 24')
    svg.setAttribute('width', '12')
    svg.setAttribute('height', '12')
    svg.setAttribute('aria-hidden', 'true')
    ICONS[name].forEach(function (d) {
      var p = document.createElementNS(SVG, 'path')
      p.setAttribute('d', d)
      svg.appendChild(p)
    })
    return svg
  }

  // K2's drawer clock: 0:05, 1:14, 1:02:09.
  function stripClock(ms) {
    var total = Math.max(0, Math.floor(ms / 1000))
    var h = Math.floor(total / 3600)
    var m = Math.floor((total % 3600) / 60)
    var sec = String(total % 60).padStart(2, '0')
    return h ? h + ':' + String(m).padStart(2, '0') + ':' + sec : m + ':' + sec
  }

  function plural(n, one, many) {
    return n + ' ' + (n === 1 ? one : many)
  }

  function countTags(c) {
    if (!c || typeof c !== 'object') return []
    var out = []
    COUNTS.forEach(function (k) {
      var n = c[k[0]]
      if (typeof n === 'number' && n > 0) out.push({ key: k[0], n: n, words: plural(n, k[1], k[2]) })
    })
    return out
  }

  function toneNow() {
    var turn = lastView && lastView.address === current.address ? lastView.turn : null
    var row = current.address ? rows.find(function (x) { return x.address === current.address }) : null
    var tone = ''
    var since = 0
    if (turn && turn.state === 'needs-you') { tone = 'needs-you'; since = turn.since }
    else if (turn && (turn.state === 'working' || turn.state === 'monitoring')) { tone = turn.state; since = turn.since }
    else if (row && (row.needsYou || row.activity === 'needs-you')) tone = 'needs-you'
    else if (row && (row.working || row.activity === 'working')) tone = 'working'
    else if (row && row.activity === 'monitoring') tone = 'monitoring'
    var counts = (turn && turn.counts) || (row && row.counts) || null
    return { tone: tone, since: since, counts: counts }
  }

  function lastSeq() {
    var items = (lastView && lastView.items) || []
    var max = null
    items.forEach(function (it) { if (typeof it.seq === 'number' && (max === null || it.seq > max)) max = it.seq })
    return max
  }

  // A reply after your post ends "Sent" (your own post coming back doesn't).
  function noteReplies(view) {
    if (!pending) return
    var items = view.items || []
    for (var i = 0; i < items.length; i++) {
      var it = items[i]
      if (typeof it.seq === 'number' && pending.seq != null && it.seq > pending.seq && !isMine((it && it.doc) || {})) { pending = null; return }
    }
  }

  function paintStrip() {
    var now = Date.now()
    var t = toneNow()
    if (t.tone) {
      sawWorking = true
      pending = null
    } else if (sawWorking) {
      sawWorking = false // went idle: the turn is over
      pending = null
    }
    if (pending && now - pending.since > PENDING_MAX_MS) pending = null
    var tone = t.tone
    var text = ''
    var since = t.since
    if (tone === 'needs-you') text = 'Needs you'
    else if (tone === 'working') text = 'Working'
    else if (tone === 'monitoring') text = 'Monitoring'
    else if (pending) { tone = 'pending'; text = 'Sent'; since = pending.since }
    if (!tone || !open) {
      turnEl.hidden = true
      turnEl.replaceChildren()
      localSince = { tone: '', at: 0 }
      stopTick()
      return
    }
    if (!since) {
      if (localSince.tone !== tone) localSince = { tone: tone, at: now }
      since = localSince.at
    }
    turnEl.hidden = false
    turnEl.dataset.tone = tone
    turnEl.classList.toggle('reduced', !!(k2 && k2.motion && k2.motion.reduced))
    turnEl.replaceChildren()
    var dot = el('span', 'ear-dot')
    dot.setAttribute('aria-hidden', 'true')
    turnEl.appendChild(dot)
    turnEl.appendChild(el('span', 'ear-label', tone === 'pending' ? text + ' · waiting…' : text))
    var tags = tone === 'pending' ? [] : countTags(t.counts)
    if (tags.length) {
      var box = el('span', 'ear-counts')
      tags.forEach(function (g) {
        var tag = el('span', 'ear-tag tag-' + g.key)
        tag.title = g.words
        tag.setAttribute('data-count', g.key)
        tag.appendChild(icon(g.key))
        tag.appendChild(document.createTextNode(String(g.n)))
        box.appendChild(tag)
      })
      turnEl.appendChild(box)
    }
    var clock = el('span', 'ear-clock', stripClock(now - since))
    clock.title = tone === 'pending' ? 'since you sent it' : 'since this turn started'
    turnEl.appendChild(clock)
    turnEl.setAttribute('aria-label', text + ', ' + clock.textContent + (tags.length ? ', ' + tags.map(function (g) { return g.words }).join(', ') : ''))
    startTick()
  }

  function startTick() { if (!tick) tick = setInterval(paintStrip, 1000) }
  function stopTick() { if (tick) { clearInterval(tick); tick = null } }

  function resetStrip() {
    lastView = null
    pending = null
    sawWorking = false
    localSince = { tone: '', at: 0 }
    paintStrip()
  }

  // ── Rooms ─────────────────────────────────────────────────────────────────
  function closeRoom(text) {
    if (offThread) { try { offThread() } catch (e) { /* already closed */ } offThread = null }
    current.address = ''
    postForm.hidden = true
    sendBtn.disabled = true
    updateSend()
    resetStrip()
    note(text, 'setup')
  }

  function openRoom(row) {
    current.label = row.label || row.address
    sendBtn.disabled = !k2.can('thread.post')
    if (current.address === row.address && offThread) return
    if (offThread) { try { offThread() } catch (e) { /* already closed */ } offThread = null }
    current.address = row.address
    resetStrip()
    textEl.placeholder = 'Message ' + current.label
    textEl.setAttribute('aria-label', 'Message ' + current.label)
    updateSend()
    note('Opening…', 'dock-note')
    postForm.hidden = !k2.can('thread.post')
    if (!k2.can('thread.subscribe')) {
      note('This widget can’t read the Thread.', 'setup')
      return
    }
    offThread = k2.thread.subscribe(row.address, drawView, function (e) {
      note('Thread unavailable: ' + errText(e), 'setup')
    })
  }

  function answerChoice(id, answer) {
    if (!current.address || !id) return
    k2.thread.answer(current.address, id, answer).catch(function (e) { dockErr('answer failed: ' + errText(e)) })
  }

  // Send: the round button, or Enter. One post at a time; the text stays
  // in the field until K2 took it, so a refused post loses nothing.
  postForm.addEventListener('submit', function (e) {
    e.preventDefault()
    var text = (textEl.value || '').trim()
    if (!text || !current.address || posting) return
    var to = current.address
    posting = true
    updateSend()
    pending = { since: Date.now(), seq: lastSeq() }
    paintStrip()
    k2.thread.post(to, text).then(function (posted) {
      posting = false
      if (pending && to === current.address && posted && typeof posted.seq === 'number') pending.seq = posted.seq
      textEl.value = ''
      growCompose()
      updateSend()
    }, function (err) {
      posting = false
      pending = null
      paintStrip()
      updateSend()
      dockErr('post failed: ' + errText(err))
    })
  })

  // The round Send takes the accent while there is something to send.
  function updateSend() {
    var ready = !!(textEl.value || '').trim() && !!current.address && !posting
    sendRound.disabled = !ready
    sendRound.classList.toggle('ready', ready)
  }

  function growCompose() {
    textEl.style.height = 'auto'
    var cap = 220
    var next = Math.min(cap, Math.max(22, textEl.scrollHeight))
    textEl.style.height = next + 'px'
    textEl.style.overflow = textEl.scrollHeight > cap ? 'auto' : 'hidden'
  }
  textEl.addEventListener('input', function () {
    growCompose()
    updateSend()
  })
  textEl.addEventListener('keydown', function (e) {
    if (e.key !== 'Enter' || e.shiftKey || e.isComposing || e.keyCode === 229) return
    e.preventDefault()
    if (typeof postForm.requestSubmit === 'function') postForm.requestSubmit()
    else postForm.dispatchEvent(new Event('submit', { cancelable: true, bubbles: true }))
  })

  sendBtn.addEventListener('click', function () {
    if (!current.address || !board) return
    var label = current.label || current.address
    k2.thread.post(current.address, board.exportMarkdown()).then(function () {
      board.toast('Sent the board to ' + label)
    }, function (e) {
      board.toast('Send failed: ' + errText(e), true)
    })
  })

  // Panel width: drag the left edge; kept until a reload.
  ;(function bindWidth() {
    var handle = document.getElementById('dockSplit')
    var startX = 0
    var startW = 0
    function clampW(w) {
      return Math.max(240, Math.min(Math.round(window.innerWidth * 0.6), w))
    }
    handle.addEventListener('pointerdown', function (e) {
      if (e.button != null && e.button !== 0) return
      e.preventDefault()
      startX = e.clientX
      startW = dock.getBoundingClientRect().width
      handle.classList.add('dragging')
      try { handle.setPointerCapture(e.pointerId) } catch (err) { /* not ours */ }
    })
    handle.addEventListener('pointermove', function (e) {
      if (!handle.classList.contains('dragging')) return
      dock.style.setProperty('--dock-w', clampW(startW + (startX - e.clientX)) + 'px')
    })
    function stop() { handle.classList.remove('dragging') }
    handle.addEventListener('pointerup', stop)
    handle.addEventListener('pointercancel', stop)
  })()

  function rowFor(address) {
    var r = rows.find(function (x) { return x.address === address })
    return r && r.state !== 'unreachable' && r.activity !== 'unreachable' ? r : null
  }

  function showFallback(text) {
    fallbackEl.hidden = !text
    fallbackEl.textContent = text || ''
  }

  // Open the picked agent's Thread. An agent that went away stays picked
  // (it comes back when it's reachable again); nothing is picked for you.
  function apply() {
    if (picker) picker.setRows(rows, chosen)
    if (!chosen) {
      showFallback('')
      closeRoom('Pick an agent above to talk to it here.')
      return
    }
    var row = rowFor(chosen)
    if (!row) {
      var gone = rows.find(function (x) { return x.address === chosen })
      var name = (gone && gone.label) || seenLabels[chosen] || chosen
      closeRoom(name + ' isn’t reachable right now.')
      return
    }
    showFallback('')
    openRoom(row)
  }

  function choose(address) {
    if (!address || address === chosen) return
    chosen = address
    remember()
    if (started) apply()
  }

  function start() {
    if (started) return
    started = true
    picker = window.KanbanPicker.create({
      root: document.getElementById('dockPick'),
      onPick: choose
    })
    note('Looking for your agents…', 'dock-note')
    k2.agents.subscribe(function (list) {
      rows = Array.isArray(list) ? list.filter(function (r) { return r && typeof r.address === 'string' && r.address }) : []
      // Labels for the agents listed now, plus the picked one's last known
      // name (it may have gone away); rebuilt each time, so it can't grow.
      var labels = {}
      if (chosen && seenLabels[chosen]) labels[chosen] = seenLabels[chosen]
      rows.forEach(function (r) { if (r.label) labels[r.address] = r.label })
      seenLabels = labels
      apply()
      paintStrip()
    }, function (e) {
      note('Agents unavailable: ' + errText(e), 'setup')
    })
  }

  function setOpen(next, save) {
    open = !!next
    dock.hidden = !open
    document.body.classList.toggle('dock-open', open)
    toggle.setAttribute('aria-pressed', open ? 'true' : 'false')
    if (open) start()
    paintStrip()
    if (save) remember()
    if (board) board.shareToolbar()
  }

  toggle.addEventListener('click', function () { setOpen(!open, true) })

  // For tests and the Garden file's seed.
  window.KanbanDock = {
    look: function () { return document.documentElement.classList.contains('look-garden') ? 'garden' : 'companion' },
    isOpen: function () { return open },
    chosen: function () { return chosen },
    address: function () { return current.address }
  }

  if (!k2) return
  k2.connected.then(function () {
    // K2 Companion's look is the default (kanban.css); a Garden file can
    // ask for the Garden theme's colours instead:
    // [widget.props.config] look = "garden".
    if (k2.config && k2.config.look === 'garden') document.documentElement.classList.add('look-garden')
    if (!k2.can('agents.subscribe') || !k2.can('thread.subscribe')) return
    toggle.hidden = false
    var seed = k2.config && typeof k2.config.agent === 'string' && k2.config.agent.indexOf('::') > 0 ? k2.config.agent : ''
    var load = canSave() ? k2.save.get(saveKey()).catch(function () { return null }) : Promise.resolve(null)
    return load.then(function (saved) {
      var s = saved && typeof saved === 'object' ? saved : {}
      chosen = typeof s.agent === 'string' && s.agent ? s.agent : seed
      setOpen(s.open === true, false)
    })
  }).catch(function () {
    // The board says when K2 didn't start the widget.
  }).then(function () {
    if (board) board.shareToolbar()
  })
})()
