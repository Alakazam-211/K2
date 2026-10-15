// The Thread panel beside the board:
// talk to one of your agents next to the board, and Send board posts the
// board as markdown. Optional: closed until you press Thread, and no agent
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

  var current = { address: '', label: '' }
  var offThread = null
  var rows = []
  var chosen = ''
  var seenLabels = {}
  var picker = null
  var started = false
  var open = false

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
  function renderItem(item) {
    var doc = (item && item.doc) || {}
    var id = item.id || doc.id || ''
    var kind = doc.kind || 'text'
    var wrap = el('div', 'msg' + (kind === 'choice' || kind === 'secret' ? ' cardish' : ''))
    if (id) wrap.setAttribute('data-msg-id', id)
    wrap.appendChild(el('div', 'from', doc.from || ''))

    if (kind === 'choice') {
      var c = doc.choice || {}
      var prompt = el('div', 'body')
      fillMarkdown(prompt, c.prompt || doc.body || '')
      wrap.appendChild(prompt)
      if (c.status === 'pending') {
        var opts = el('div', 'choice-opts')
        ;(c.options || []).forEach(function (opt) {
          var text = opt == null ? '' : typeof opt === 'string' ? opt : String(opt.label || opt.value || '')
          if (!text) return
          var b = el('button', null, text)
          b.type = 'button'
          b.addEventListener('click', function () { answerChoice(id, text) })
          opts.appendChild(b)
        })
        wrap.appendChild(opts)
        if (c.allow_custom) {
          var row = el('div', 'secret-row')
          var inp = el('input')
          inp.type = 'text'
          inp.placeholder = 'Custom answer'
          var go = el('button', null, 'Answer')
          go.type = 'button'
          go.addEventListener('click', function () {
            var t = (inp.value || '').trim()
            if (t) answerChoice(id, t)
          })
          row.appendChild(inp)
          row.appendChild(go)
          wrap.appendChild(row)
        }
      } else {
        wrap.appendChild(el('div', 'status', c.status === 'voided' ? 'voided' : 'answered' + (c.answer ? ': ' + c.answer : '')))
      }
      return wrap
    }

    if (kind === 'secret') {
      wrap.appendChild(el('div', 'body', doc.body || 'Secret requested'))
      wrap.appendChild(el('div', 'status', 'Answer secret cards in K2’s own Thread (widgets can’t send secrets).'))
      return wrap
    }

    var body = el('div', 'body')
    fillMarkdown(body, doc.body || '')
    wrap.appendChild(body)
    return wrap
  }

  function drawView(view) {
    if (!view || view.address !== current.address) return
    var atBottom = bodyEl.scrollHeight - bodyEl.scrollTop - bodyEl.clientHeight < 40
    if (view.phase === 'unavailable' || view.phase === 'failed') {
      note(view.note || 'This Thread isn’t available right now.', 'setup')
      return
    }
    bodyEl.replaceChildren()
    ;(view.items || []).forEach(function (item) { bodyEl.appendChild(renderItem(item)) })
    if (!(view.items || []).length) note(view.phase === 'opening' ? 'Opening…' : 'No Thread messages yet.', 'dock-note')
    lastView = view
    noteReplies(view)
    paintStrip()
    if (atBottom) bodyEl.scrollTop = bodyEl.scrollHeight
  }

  // ── Working strip ─────────────────────────────────────────────────────────
  // After K2's Thread working strip: a left rule and a square mark in the
  // tone's colour, the state word, then "· 12s". Live only; driven by the
  // open Thread's `turn` and the picked agent's row. After a post it says
  // "Sent · waiting…" until the agent reports working or a reply lands.
  var PENDING_MAX_MS = 3 * 60 * 1000
  var lastView = null
  var pending = null // { since, seq }
  var localSince = { tone: '', at: 0 }
  var sawWorking = false
  var tick = null

  function stripClock(ms) {
    var total = Math.max(0, Math.floor(ms / 1000))
    var m = Math.floor(total / 60)
    var sec = total % 60
    return m ? m + 'm ' + String(sec).padStart(2, '0') + 's' : sec + 's'
  }

  function plural(n, one, many) {
    return n + ' ' + (n === 1 ? one : many)
  }

  function countBits(c) {
    if (!c || typeof c !== 'object') return []
    var out = []
    if (typeof c.subagents === 'number' && c.subagents > 0) out.push(plural(c.subagents, 'subagent', 'subagents'))
    if (typeof c.tools === 'number' && c.tools > 0) out.push(plural(c.tools, 'tool', 'tools'))
    if (typeof c.commands === 'number' && c.commands > 0) out.push(plural(c.commands, 'command', 'commands'))
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

  function noteReplies(view) {
    if (!pending) return
    var items = view.items || []
    for (var i = 0; i < items.length; i++) {
      var it = items[i]
      if (typeof it.seq === 'number' && pending.seq != null && it.seq > pending.seq) { pending = null; return }
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
    else if (tone === 'working') text = 'Working…'
    else if (tone === 'monitoring') text = 'Monitoring…'
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
    var head = el('div', 'strip-head')
    var mark = el('span', 'strip-mark')
    mark.setAttribute('aria-hidden', 'true')
    head.appendChild(mark)
    head.appendChild(el('span', 'strip-label', text))
    var clock = el('span', 'strip-since', tone === 'pending' ? '· waiting… ' + stripClock(now - since) : '· ' + stripClock(now - since))
    clock.title = tone === 'pending' ? 'since you sent it' : 'since this turn started'
    head.appendChild(clock)
    turnEl.appendChild(head)
    var bits = tone === 'pending' ? [] : countBits(t.counts)
    if (bits.length) turnEl.appendChild(el('div', 'strip-tally', bits.join(' · ')))
    turnEl.setAttribute('aria-label', text + ' ' + clock.textContent + (bits.length ? ', ' + bits.join(', ') : ''))
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
    textEl.placeholder = 'Write to ' + current.label
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

  postForm.addEventListener('submit', function (e) {
    e.preventDefault()
    var text = (textEl.value || '').trim()
    if (!text || !current.address) return
    var to = current.address
    pending = { since: Date.now(), seq: lastSeq() }
    paintStrip()
    k2.thread.post(to, text).then(function (posted) {
      if (pending && to === current.address && posted && typeof posted.seq === 'number') pending.seq = posted.seq
      textEl.value = ''
      growCompose()
    }, function (err) {
      pending = null
      paintStrip()
      dockErr('post failed: ' + errText(err))
    })
  })

  function growCompose() {
    textEl.style.height = 'auto'
    var cap = 220
    var next = Math.min(cap, Math.max(28, textEl.scrollHeight))
    textEl.style.height = next + 'px'
    textEl.style.overflow = textEl.scrollHeight > cap ? 'auto' : 'hidden'
  }
  textEl.addEventListener('input', growCompose)
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
      rows.forEach(function (r) { if (r.label) seenLabels[r.address] = r.label })
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
    isOpen: function () { return open },
    chosen: function () { return chosen },
    address: function () { return current.address }
  }

  if (!k2) return
  k2.connected.then(function () {
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
