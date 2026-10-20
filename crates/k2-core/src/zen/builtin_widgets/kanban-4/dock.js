// The Thread panel beside the board (k2:kanban@4: @3's panel with the
// Thread's files and links, as K2's own Thread has them):
//   - files on a message: a chip each (name, size; "changed since sent",
//     "moved or deleted"; a file not shared with you reads "A file (not
//     shared with you)" with no name); an image (PNG, JPEG, GIF, WebP,
//     HEIC, SVG, BMP, TIFF) shows as the picture, K2's thumbnail drawn only
//     through an <img> with a data: URL. A click opens it full size in K2;
//   - file paths printed in a message that lead into the agent's workspace
//     are links (K2 decides which); a click opens the file at its line;
//   - the message box takes files: + opens K2's own picker, the picks wait
//     as removable chips and ride the post (files alone are a message);
//   - reading the newest message (panel open, visible, at the bottom)
//     marks the Thread read for you; coming back to the window catches up
//     with what changed since (a delta read); scrolling to the top loads
//     earlier messages.
// Each of these asks k2.can first, so on an older K2 the panel is @3's.
//   files     k2.thread.attachment, k2.thread.open, k2.thread.pickFiles
//   links     k2.thread.links, k2.thread.open
//   read      k2.thread.markRead, k2.thread.read (afterSeq / beforeSeq)
// From @3: K2's stick-to-bottom scrolling and a "new messages" pill:
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
  // Files waiting in the message box ({relPath, name}), at most FILES_MAX.
  var picked = []
  var picking = false
  var FILES_MAX = 10

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
    var files = filesOf(id, doc)
    // Files alone are a message: no empty bubble above them.
    if (!(files && kind === 'text' && !String(doc.body || '').trim())) wrap.appendChild(bubble)

    if (kind === 'choice') {
      var c = doc.choice || {}
      var prompt = el('div', 'body')
      fillMarkdown(prompt, c.prompt || doc.body || '')
      linkify(prompt)
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
      linkify(body)
      bubble.appendChild(body)
    }
    if (files) wrap.appendChild(files)

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

  // ── Files on messages ─────────────────────────────────────────────────────
  // K2 sends each file as {kind, state, name?, size?, mime?, line?, ...};
  // never where it lives. Opening goes through K2 (k2.thread.open).
  var IMAGE_EXT = /\.(png|jpe?g|gif|webp|heic|heif|svg|bmp|tiff?)$/i
  var IMAGE_MIME = /^image\/(png|jpeg|gif|webp|svg\+xml|bmp|tiff|heic|heif)$/
  var B64 = /^[A-Za-z0-9+/]+={0,2}$/
  var THUMBS_MAX = 40
  var thumbs = {}
  var thumbKeys = []

  function isImage(f) {
    return /^image\//.test(String(f.mime || '')) || IMAGE_EXT.test(String(f.name || ''))
  }

  // 245812 -> "240 KB".
  function sizeText(n) {
    if (typeof n !== 'number' || !isFinite(n) || n < 0) return ''
    if (n < 1024) return n + ' B'
    if (n < 1048576) return Math.round(n / 1024) + ' KB'
    return (n / 1048576).toFixed(1).replace(/\.0$/, '') + ' MB'
  }

  function openFile(target) {
    if (!current.address || !k2.can('thread.open')) return
    k2.thread.open(current.address, target).then(function (r) {
      if (!r || !r.opened) dockErr('That file can’t be opened here.')
    }, function (e) {
      dockErr('open failed: ' + errText(e))
    })
  }

  function filesOf(id, doc) {
    var list = Array.isArray(doc.attachments) ? doc.attachments : []
    if (!list.length) return null
    var box = el('div', 'msg-files')
    list.forEach(function (f, n) {
      if (f && typeof f === 'object') box.appendChild(fileNode(id, f, n))
    })
    return box
  }

  function fileNode(id, f, n) {
    var state = String(f.state || 'ok')
    if (state === 'hidden') {
      var h = el('span', 'file-chip hidden')
      h.setAttribute('data-file-state', 'hidden')
      h.appendChild(el('span', 'file-name', 'A file (not shared with you)'))
      return h
    }
    if (id && isImage(f) && (state === 'ok' || state === 'changed') && k2.can('thread.attachment')) return thumbNode(id, f, n)
    return chipNode(id, f, n)
  }

  function chipNode(id, f, n) {
    var state = String(f.state || 'ok')
    var gone = state === 'gone'
    var name = String(f.name || 'file') + (typeof f.line === 'number' ? ':' + f.line : '')
    var chip = el('button', 'file-chip ' + state)
    chip.type = 'button'
    chip.setAttribute('data-file-state', state)
    chip.appendChild(el('span', 'file-name', name))
    var note = gone ? 'moved or deleted' : state === 'changed' ? 'changed since sent' : sizeText(f.size)
    if (note) chip.appendChild(el('span', 'file-note', note))
    chip.title = gone ? name + ' was moved or deleted' : 'Open ' + name
    if (gone || !id || !k2.can('thread.open')) chip.disabled = true
    else chip.addEventListener('click', function () { openFile({ docId: id, n: n }) })
    return chip
  }

  // A thumbnail's data: URL, only for an image type and clean base64.
  function thumbSrc(t) {
    var mime = String((t && t.mime) || '')
    var b64 = t && typeof t.base64 === 'string' ? t.base64 : ''
    if (!IMAGE_MIME.test(mime) || !b64 || !B64.test(b64)) return ''
    return 'data:' + mime + ';base64,' + b64
  }

  // One request per picture (cached, a few dozen kept); '' when K2 has none.
  function loadThumb(address, id, f, n, done) {
    var key = address + '\n' + id + '\n' + n + '\n' + (f.modifiedAt || '')
    var have = thumbs[key]
    if (have && have.wait) return have.wait.push(done)
    if (have) return Promise.resolve(have.src).then(done)
    var entry = { wait: [done], src: '' }
    thumbs[key] = entry
    thumbKeys.push(key)
    while (thumbKeys.length > THUMBS_MAX) delete thumbs[thumbKeys.shift()]
    k2.thread.attachment(address, id, n, { mode: 'thumb', px: 512 }).then(thumbSrc, function () { return '' }).then(function (src) {
      var waiting = entry.wait
      entry.wait = null
      entry.src = src
      waiting.forEach(function (fn) { fn(src) })
    })
  }

  function thumbNode(id, f, n) {
    var name = String(f.name || 'image')
    var fig = el('button', 'file-thumb')
    fig.type = 'button'
    fig.setAttribute('data-file-state', String(f.state || 'ok'))
    fig.title = 'Open ' + name
    fig.setAttribute('aria-label', 'Open ' + name)
    var wait = el('span', 'thumb-wait')
    fig.appendChild(wait)
    fig.appendChild(el('span', 'file-cap', name + (f.state === 'changed' ? ' · changed since sent' : '')))
    function asChip() {
      if (fig.parentNode) fig.parentNode.replaceChild(chipNode(id, f, n), fig)
    }
    loadThumb(current.address, id, f, n, function (src) {
      if (!src) return asChip()
      var img = el('img')
      img.alt = name
      img.addEventListener('error', asChip)
      img.src = src
      if (wait.parentNode === fig) fig.replaceChild(img, wait)
    })
    if (k2.can('thread.open')) fig.addEventListener('click', function () { openFile({ docId: id, n: n }) })
    else fig.disabled = true
    return fig
  }

  // ── File links in messages ────────────────────────────────────────────────
  // Path-shaped text (src/app.ts:12, ./notes.md, an absolute path) is only
  // a candidate; K2 says which lead into the agent's workspace
  // (k2.thread.links), the rest stay plain text. Never inside a code block,
  // a link's URL or a fenced block.
  var PATH_RE = /(^|[\s(\[{<"'*_,;|>])((?:~\/|\.{1,2}\/|\/)?(?:[A-Za-z0-9._@+%=~-]+\/)+[A-Za-z0-9._@+%=~-]+\/?)(:\d{1,7}(?::\d{1,7})?)?/g
  var linkAnswers = {}
  var linkCount = 0
  var linkQueue = []
  var linkTimer = null

  function canLink() {
    return !!(k2 && typeof k2.can === 'function' && k2.can('thread.links') && k2.can('thread.open'))
  }

  function plausible(p, hasLine) {
    if (p.indexOf('//') >= 0) return false
    var segs = p.replace(/^~\//, '').split('/').filter(Boolean)
    if (/^\.{1,2}\//.test(p)) return segs.some(function (s) { return s !== '.' && s !== '..' })
    if (segs.length < 2) return false
    if (p.charAt(0) === '/' || p.indexOf('~/') === 0) return true
    if (/\.(com|org|net|io|dev|ai|app|co)$/i.test(segs[0])) return false
    return /[^./]\.[A-Za-z][A-Za-z0-9_-]{0,9}$/.test(segs[segs.length - 1]) || /\/$/.test(p) || hasLine
  }

  function pathHits(text) {
    var out = []
    var m
    PATH_RE.lastIndex = 0
    while ((m = PATH_RE.exec(text)) && out.length < 50) {
      var raw = m[2]
      var path = raw.replace(/\.+$/, '')
      var suffix = path === raw ? m[3] || '' : ''
      if (!plausible(path, !!suffix)) continue
      var start = m.index + m[1].length
      out.push({ start: start, end: start + path.length + suffix.length, path: path, line: suffix ? parseInt(suffix.slice(1), 10) : 0 })
    }
    return out
  }

  function linkify(root) {
    if (!canLink()) return
    var found = []
    function mark(text, path, line) {
      var s = el('span', 'file-cand', text)
      s.setAttribute('data-cand', line ? path + ':' + line : path)
      if (line) s.setAttribute('data-line', String(line))
      found.push(s)
      return s
    }
    function splitText(t) {
      var text = t.nodeValue || ''
      var hits = pathHits(text)
      if (!hits.length) return
      var frag = document.createDocumentFragment()
      var at = 0
      hits.forEach(function (x) {
        if (x.start > at) frag.appendChild(document.createTextNode(text.slice(at, x.start)))
        frag.appendChild(mark(text.slice(x.start, x.end), x.path, x.line))
        at = x.end
      })
      if (at < text.length) frag.appendChild(document.createTextNode(text.slice(at)))
      t.parentNode.replaceChild(frag, t)
    }
    // Inline code that is a path and nothing else (`Cargo.toml`, `src/a.ts:3`).
    function codeCand(code) {
      var text = (code.textContent || '').trim()
      var m = /^([^\s:]+)(?::(\d{1,7}))?(?::\d{1,7})?$/.exec(text)
      if (!m || text.length > 300 || /^[a-z][a-z0-9+.-]*:/i.test(text)) return
      if (m[1].indexOf('/') < 0 && !/[^./]\.[A-Za-z][A-Za-z0-9_-]{0,9}$/.test(m[1])) return
      var s = mark('', m[1], m[2] ? parseInt(m[2], 10) : 0)
      code.parentNode.replaceChild(s, code)
      s.appendChild(code)
    }
    function walk(node) {
      Array.prototype.slice.call(node.childNodes).forEach(function (n) {
        if (n.nodeType === 3) return splitText(n)
        if (n.nodeType !== 1) return
        if (n.tagName === 'PRE' || n.tagName === 'A' || n.classList.contains('md-url')) return
        if (n.tagName === 'CODE') return codeCand(n)
        walk(n)
      })
    }
    walk(root)
    found.forEach(askLink)
  }

  function askLink(span) {
    var input = span.getAttribute('data-cand')
    var a = linkAnswers[input]
    if (a === 'pending') return
    if (a) return paintLink(span, a)
    // A long chat: the answers start over rather than grow without end.
    if (++linkCount > 2000) {
      linkAnswers = {}
      linkCount = 1
    }
    linkAnswers[input] = 'pending'
    linkQueue.push(input)
    if (!linkTimer) linkTimer = setTimeout(flushLinks, 0)
  }

  function flushLinks() {
    linkTimer = null
    var address = current.address
    var batch = linkQueue.splice(0, 200)
    if (linkQueue.length) linkTimer = setTimeout(flushLinks, 0)
    if (!batch.length || !address) return
    k2.thread.links(address, batch).then(function (r) {
      if (address !== current.address) return
      var got = {}
      ;(r && Array.isArray(r.links) ? r.links : []).forEach(function (l) {
        if (l && typeof l.input === 'string') got[l.input] = l
      })
      batch.forEach(function (input) {
        var l = got[input]
        linkAnswers[input] = l && l.linkable && typeof l.rel === 'string' ? { rel: l.rel, kind: l.kind || 'file' } : { rel: '' }
      })
      bodyEl.querySelectorAll('.file-cand:not(.file-link)').forEach(function (s) {
        var a = linkAnswers[s.getAttribute('data-cand')]
        if (a && a !== 'pending') paintLink(s, a)
      })
    }, function () {
      if (address !== current.address) return
      batch.forEach(function (input) { linkAnswers[input] = { rel: '' } })
    })
  }

  function resetLinks() {
    linkAnswers = {}
    linkCount = 0
    linkQueue = []
  }

  function paintLink(span, a) {
    if (!a.rel) return
    span.classList.add('file-link')
    span.setAttribute('role', 'link')
    span.tabIndex = 0
    span.setAttribute('data-rel', a.rel)
    var line = span.getAttribute('data-line')
    span.title = (a.kind === 'directory' ? 'Show ' : 'Open ') + a.rel + (line ? ':' + line : '')
  }

  function followLink(e) {
    var t = e.target && e.target.closest ? e.target.closest('.file-link') : null
    if (!t || !bodyEl.contains(t)) return
    if (e.type === 'keydown' && e.key !== 'Enter' && e.key !== ' ') return
    var sel = window.getSelection ? window.getSelection() : null
    if (e.type === 'click' && sel && !sel.isCollapsed && String(sel).length > 0) return
    e.preventDefault()
    var target = { rel: t.getAttribute('data-rel') }
    var line = parseInt(t.getAttribute('data-line') || '', 10)
    if (line > 0) target.line = line
    openFile(target)
  }
  bodyEl.addEventListener('click', followLink)
  bodyEl.addEventListener('keydown', followLink)

  // The times say "5m ago"; they move on without a new message.
  function refreshTimes() {
    var now = nowSec()
    bodyEl.querySelectorAll('.time[data-at]').forEach(function (t) {
      var at = Number(t.getAttribute('data-at'))
      if (isFinite(at)) t.textContent = ago(at, now)
    })
  }
  setInterval(refreshTimes, 30000)

  // ── Scrolling (K2's stick-to-bottom, hooks/useStickToBottom.ts) ───────────
  // At the bottom (within STICK_PX), the list stays pinned to its true
  // bottom: a new message glides into view, and the working ear coming and
  // going, a reply growing or the field growing keep it there. Scrolled up,
  // nothing moves: new messages leave the position alone and a "new
  // messages" pill at the foot counts them; pressing it, or scrolling down by
  // hand, goes to the bottom and clears it. "At the bottom" changes only on
  // a scroll the person made; a scroll after the box or its content changed
  // size is the browser's clamp, not the person. Your own post always goes
  // to the bottom.
  // The list is patched, never rebuilt: a message keeps its node until its
  // contents change (WebKit, the macOS window, resets scrollTop to 0 when a
  // scroll box's content is replaced), and a changed message above the view
  // keeps the first visible message where it was.
  var STICK_PX = 32
  var SMOOTH_MS = 700
  var stuck = true
  var seenSize = null
  var unseen = 0
  var autoUntil = 0
  var settleTimer = null
  var stickNext = false
  var listEl = null
  var nodes = {}
  var known = {}
  var sizeObs = null
  var footEl = postForm.parentElement
  var newerBtn = el('button', 'dock-newer')
  newerBtn.type = 'button'
  newerBtn.hidden = true
  footEl.insertBefore(newerBtn, footEl.firstChild)

  // The message box's files: + (K2's own picker) inside the field, the
  // picks as removable chips above it.
  var composeBox = postForm.querySelector('.compose-box')
  var attachBtn = el('button', 'attach-round')
  attachBtn.type = 'button'
  attachBtn.title = 'Attach files'
  attachBtn.setAttribute('aria-label', 'Attach files')
  attachBtn.hidden = true
  ;(function plus() {
    var ns = 'http://www.w3.org/2000/svg'
    var svg = document.createElementNS(ns, 'svg')
    svg.setAttribute('viewBox', '0 0 24 24')
    svg.setAttribute('width', '17')
    svg.setAttribute('height', '17')
    svg.setAttribute('aria-hidden', 'true')
    var p = document.createElementNS(ns, 'path')
    p.setAttribute('d', 'M12 5v14M5 12h14')
    svg.appendChild(p)
    attachBtn.appendChild(svg)
  })()
  composeBox.insertBefore(attachBtn, composeBox.firstChild)
  var pickedEl = el('div', 'compose-files')
  pickedEl.hidden = true
  postForm.insertBefore(pickedEl, composeBox)

  function drawPicked() {
    pickedEl.replaceChildren()
    pickedEl.hidden = !picked.length
    picked.forEach(function (f) {
      var chip = el('span', 'compose-file')
      chip.setAttribute('data-rel', f.relPath)
      chip.title = f.relPath
      chip.appendChild(el('span', 'file-name', f.name))
      var x = el('button', null, '×')
      x.type = 'button'
      x.title = 'Remove ' + f.name
      x.setAttribute('aria-label', 'Remove ' + f.name)
      x.addEventListener('click', function () {
        var i = picked.indexOf(f)
        if (i >= 0) picked.splice(i, 1)
        drawPicked()
        updateSend()
        textEl.focus()
      })
      chip.appendChild(x)
      pickedEl.appendChild(chip)
    })
  }

  function clearPicked() {
    picked = []
    drawPicked()
  }

  attachBtn.addEventListener('click', function () {
    if (!current.address || picking || !k2.can('thread.pickFiles')) return
    var to = current.address
    picking = true
    attachBtn.disabled = true
    k2.thread.pickFiles(to).then(function (r) {
      picking = false
      attachBtn.disabled = false
      if (to !== current.address) return
      var got = r && Array.isArray(r.files) ? r.files : []
      got.forEach(function (f) {
        if (!f || typeof f.relPath !== 'string' || !f.relPath || picked.length >= FILES_MAX) return
        if (picked.some(function (p) { return p.relPath === f.relPath })) return
        picked.push({ relPath: f.relPath, name: String(f.name || f.relPath.split('/').pop()) })
      })
      drawPicked()
      updateSend()
      if (got.length) textEl.focus()
    }, function (e) {
      picking = false
      attachBtn.disabled = false
      dockErr('attach failed: ' + errText(e))
    })
  })

  function bottomOf() {
    return Math.max(0, bodyEl.scrollHeight - bodyEl.clientHeight)
  }

  function noteSize() {
    seenSize = { client: bodyEl.clientHeight, scroll: bodyEl.scrollHeight }
  }

  function reducedMotion() {
    return !!(k2 && k2.motion && k2.motion.reduced)
  }

  function showNewer() {
    newerBtn.hidden = !unseen
    var words = unseen === 1 ? '1 new message' : unseen + ' new messages'
    newerBtn.textContent = words + ' ↓'
    newerBtn.setAttribute('aria-label', words + ', go to the newest')
  }

  function clearNewer() {
    if (!unseen && newerBtn.hidden) return
    unseen = 0
    showNewer()
  }

  // Pin to the bottom: smooth for a new message, at once for anything else.
  function pin(smooth) {
    stuck = true
    clearNewer()
    if (bodyEl.clientHeight === 0) return
    var to = bottomOf()
    if (smooth && !reducedMotion() && typeof bodyEl.scrollTo === 'function') {
      autoUntil = Date.now() + SMOOTH_MS
      bodyEl.scrollTo({ top: to, behavior: 'smooth' })
      // When it lands, settle on the true bottom (sizes may have moved).
      clearTimeout(settleTimer)
      settleTimer = setTimeout(function () {
        autoUntil = 0
        sync()
      }, SMOOTH_MS + 50)
    } else if (bodyEl.scrollTop !== to) {
      bodyEl.scrollTop = to
    }
    noteSize()
  }

  // A size changed (the box, a message, the ear, the field): re-pin if pinned.
  function sync() {
    if (bodyEl.clientHeight === 0) return
    if (stuck) {
      var to = bottomOf()
      if (Date.now() < autoUntil && typeof bodyEl.scrollTo === 'function') bodyEl.scrollTo({ top: to, behavior: 'smooth' })
      else if (bodyEl.scrollTop !== to) bodyEl.scrollTop = to
    }
    noteSize()
  }

  bodyEl.addEventListener('scroll', function () {
    if (bodyEl.clientHeight === 0) return
    if (Date.now() < autoUntil) {
      // Our own smooth scroll on its way down.
      if (bottomOf() - bodyEl.scrollTop <= 1) autoUntil = 0
      noteSize()
      return
    }
    if (!seenSize || seenSize.client !== bodyEl.clientHeight || seenSize.scroll !== bodyEl.scrollHeight) {
      sync()
      return
    }
    stuck = bottomOf() - bodyEl.scrollTop <= STICK_PX
    if (stuck) {
      clearNewer()
      markRead()
    }
    maybeOlder()
  })
  // The person takes over: a smooth scroll on its way stops counting as ours.
  ;['wheel', 'touchstart', 'pointerdown', 'keydown'].forEach(function (type) {
    bodyEl.addEventListener(type, function () { autoUntil = 0 }, { passive: true })
  })
  newerBtn.addEventListener('click', function () { pin(true) })

  function watchSizes() {
    if (typeof ResizeObserver === 'undefined') return
    if (!sizeObs) sizeObs = new ResizeObserver(sync)
    sizeObs.disconnect()
    sizeObs.observe(bodyEl)
    if (listEl) sizeObs.observe(listEl)
  }

  // The first message at least partly in view, and where it sits.
  function anchorNow() {
    if (!listEl) return null
    var top = bodyEl.getBoundingClientRect().top
    var kids = listEl.children
    for (var i = 0; i < kids.length; i++) {
      var r = kids[i].getBoundingClientRect()
      if (r.bottom > top) return { node: kids[i], at: r.top - top, scrollTop: bodyEl.scrollTop }
    }
    return { node: null, at: 0, scrollTop: bodyEl.scrollTop }
  }

  function keepAnchor(a) {
    if (!a) return
    if (a.node && a.node.isConnected) {
      var moved = a.node.getBoundingClientRect().top - bodyEl.getBoundingClientRect().top - a.at
      if (moved) bodyEl.scrollTop += moved
    } else if (bodyEl.scrollTop !== a.scrollTop) {
      bodyEl.scrollTop = a.scrollTop
    }
    noteSize()
  }

  function forgetList() {
    listEl = null
    nodes = {}
  }

  function resetScroll() {
    forgetList()
    known = {}
    stuck = true
    stickNext = false
    autoUntil = 0
    clearNewer()
  }

  function keyOf(item, i) {
    var doc = (item && item.doc) || {}
    return String(item.id || doc.id || (typeof item.seq === 'number' ? 'seq:' + item.seq : 'i:' + i))
  }

  function drawView(view) {
    if (!view || view.address !== current.address) return
    if (view.phase === 'unavailable' || view.phase === 'failed') {
      forgetList()
      note(view.note || 'This Thread isn’t available right now.', 'setup')
      return
    }
    var now = nowSec()
    var items = view.items || []
    // Earlier messages loaded above (paging back) are never "new".
    var prevMax = lastSeq()
    if (!items.length) {
      forgetList()
      note(view.phase === 'opening' ? 'Opening…' : 'Message ' + (current.label || 'this agent') + '. It reads this like a text.', 'dock-note dock-empty')
    } else {
      var first = !listEl || !listEl.isConnected
      var anchor = first || stuck ? null : anchorNow()
      if (first) {
        listEl = el('div', 'msgs')
        nodes = {}
        bodyEl.replaceChildren(listEl)
      }
      var keep = {}
      var seenNow = {}
      var prev = null
      var fresh = 0
      items.forEach(function (item, i) {
        var doc = (item && item.doc) || {}
        var key = keyOf(item, i)
        var mine = isMine(doc)
        var sig = JSON.stringify([item.seq, doc, authorOf(doc, mine)])
        var had = nodes[key]
        var node = had && had.sig === sig ? had.node : renderItem(item, now)
        if (had && had.node !== node) listEl.replaceChild(node, had.node)
        keep[key] = { node: node, sig: sig }
        var want = prev ? prev.nextSibling : listEl.firstChild
        if (want !== node) listEl.insertBefore(node, want)
        prev = node
        // New since the last draw: counts toward the pill (yours never do).
        var older = prevMax !== null && typeof item.seq === 'number' && item.seq <= prevMax
        if (!known[key] && !first && !mine && !older) fresh++
        seenNow[key] = true
      })
      Object.keys(nodes).forEach(function (key) {
        if (!keep[key] && nodes[key].node.parentNode === listEl) listEl.removeChild(nodes[key].node)
      })
      nodes = keep
      // The keys drawn now, rebuilt each time, so it can't grow.
      known = seenNow
      keepAnchor(anchor)
      watchSizes()
      if (first || stickNext) {
        pin(!first)
        stickNext = false
      } else if (stuck) {
        pin(fresh > 0)
      } else if (fresh) {
        unseen += fresh
        showNewer()
      }
    }
    lastView = view
    noteReplies(view)
    paintStrip()
    markRead()
  }

  // ── Read state, catching up, earlier messages ─────────────────────────────
  // Read: the panel is open, the window shows it, and you are at the newest
  // message; forward only, once per newer message (k2.thread.markRead).
  var readTo = { address: '', seq: -1 }
  function markRead() {
    if (!open || !current.address || !stuck || !k2 || !k2.can('thread.markRead')) return
    if (document.visibilityState && document.visibilityState !== 'visible') return
    var seq = lastSeq()
    if (seq === null) return
    if (readTo.address === current.address && seq <= readTo.seq) return
    readTo = { address: current.address, seq: seq }
    k2.thread.markRead(current.address, { seq: seq }).catch(function () {
      // Not marked: K2's own Thread still marks it when you read it there.
    })
  }

  // Coming back (the window shown again, the network back): what changed
  // since the newest message here, as a delta (k2.thread.read afterSeq).
  var catching = false
  function catchUp() {
    if (!open || !current.address || catching || !lastView || !k2.can('thread.read')) return
    var after = lastSeq()
    if (after === null) return
    var at = current.address
    catching = true
    k2.thread.read(at, { afterSeq: after }).then(function (d) {
      catching = false
      if (at !== current.address || !lastView || !d || !Array.isArray(d.items)) return
      var merged = mergeDelta(lastView.items || [], d)
      if (merged) drawView(Object.assign({}, lastView, { items: merged }))
      if (d.hasMore === true) catchUp()
    }, function () {
      catching = false
    })
  }

  // New items join, changed ones are replaced where they are, removed go.
  function mergeDelta(items, d) {
    var byId = {}
    var order = []
    var changed = false
    items.forEach(function (it) {
      if (it && it.id) {
        byId[it.id] = it
        order.push(it.id)
      }
    })
    ;(Array.isArray(d.removed) ? d.removed : []).forEach(function (r) {
      if (r && byId[r.id]) {
        delete byId[r.id]
        changed = true
      }
    })
    ;(Array.isArray(d.changed) ? d.changed : []).forEach(function (it) {
      if (it && it.id && byId[it.id]) {
        byId[it.id] = it
        changed = true
      }
    })
    d.items.forEach(function (it) {
      if (!it || !it.id) return
      if (!byId[it.id]) order.push(it.id)
      byId[it.id] = it
      changed = true
    })
    if (!changed) return null
    return order
      .filter(function (id, i) { return byId[id] && order.indexOf(id) === i })
      .map(function (id) { return byId[id] })
      .sort(function (a, b) { return (a.seq || 0) - (b.seq || 0) })
  }

  document.addEventListener('visibilitychange', function () {
    if (document.visibilityState !== 'visible') return
    catchUp()
    markRead()
  })
  window.addEventListener('online', catchUp)

  // Scrolled to the top with earlier messages left: load the page before
  // the first one (k2.thread.read beforeSeq); what you were reading stays put.
  var loadingOlder = false
  function maybeOlder() {
    if (loadingOlder || !lastView || !lastView.hasMore || !current.address || !k2.can('thread.read')) return
    if (bodyEl.scrollTop > 40) return
    var first = null
    ;(lastView.items || []).forEach(function (it) {
      if (typeof it.seq === 'number' && (first === null || it.seq < first)) first = it.seq
    })
    if (first === null) return
    var at = current.address
    loadingOlder = true
    k2.thread.read(at, { beforeSeq: first }).then(function (v) {
      loadingOlder = false
      if (at !== current.address || !v || !Array.isArray(v.items) || !lastView) return
      // K2's own Thread answers the whole live view; a plain page joins ours.
      if (v.address === at) return drawView(v)
      var merged = mergeDelta(lastView.items || [], { items: v.items })
      if (merged) drawView(Object.assign({}, lastView, { items: merged, hasMore: v.hasMore === true }))
    }, function () {
      loadingOlder = false
    })
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
    resetScroll()
    resetLinks()
    clearPicked()
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
    resetScroll()
    resetStrip()
    // Links and picked files belong to one agent's workspace.
    resetLinks()
    clearPicked()
    attachBtn.hidden = !k2.can('thread.pickFiles')
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
    var files = picked.slice()
    if ((!text && !files.length) || !current.address || posting) return
    var to = current.address
    posting = true
    updateSend()
    stickNext = true
    pin(true)
    pending = { since: Date.now(), seq: lastSeq() }
    paintStrip()
    // Picked files ride the post; the text and chips stay until K2 took it.
    var post = files.length
      ? k2.thread.post(to, text, { attachments: files.map(function (f) { return { relPath: f.relPath } }) })
      : k2.thread.post(to, text)
    post.then(function (posted) {
      posting = false
      if (pending && to === current.address && posted && typeof posted.seq === 'number') pending.seq = posted.seq
      textEl.value = ''
      if (to === current.address) {
        picked = picked.filter(function (f) { return files.indexOf(f) < 0 })
        drawPicked()
      }
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
    var ready = (!!(textEl.value || '').trim() || picked.length > 0) && !!current.address && !posting
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
    stickNext = true
    pin(true)
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
    if (open) markRead()
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
