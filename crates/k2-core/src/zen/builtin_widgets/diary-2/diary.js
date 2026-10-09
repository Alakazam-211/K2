// k2:diary@2 — the haunted Diary (Rosson 2026-10-08; prd-zen-user-widgets-v2
// UWB24). One aged page per agent on THIS computer, and nothing else to
// choose: grab a page's corner and turn it (drag it, click it, or use the
// arrow keys) to write to another agent. What you write sinks into the
// paper; the reply arrives whole (Thread doesn't stream) and bleeds back in
// handwriting at a natural pace, capped near 6 s, and a tap shows it all.
// While the agent works, the ink stirs; when it needs you, a bookmark lifts
// and the corner toward its page smoulders. Whispers only when you turn
// them on. Reduced motion stills every effect.
//
// The widget is the journal only (Rosson 2026-10-08): the room around it is
// the Garden's theme (`haunted`). Only this computer's agents get a page:
// the list keeps `<handle>::local` rows, so a remote agent on one of your
// Homes never gets one.
//
// Words read as markdown, built node by node (never an HTML string); your
// own words stay dark ink on the page once sent. When the pen is empty, a
// ghost writes teases into it (a placeholder only, never sent).
//
// Rules this file keeps (the k2-zen skill's): agent text only ever goes
// through textContent; no inline handlers; no network or storage.
(function () {
  'use strict'

  var LOCAL_HOST = 'local'
  var REVEAL_MIN_MS = 900
  var REVEAL_MAX_MS = 6000
  var REVEAL_MS_PER_CHAR = 38
  var WET_CHARS = 7
  var OLDER_PAGE = 50
  var TURN_MS = 700
  var TURN_COMMIT = 0.33
  var DRAG_START_PX = 6

  var $ = function (id) {
    var el = document.getElementById(id)
    if (!el) throw new Error('diary: missing #' + id)
    return el
  }
  var dom = {
    root: document.documentElement,
    desk: $('desk'),
    book: $('book'),
    page: $('page'),
    caption: $('caption'),
    who: $('who'),
    flourish: $('flourish'),
    mood: $('mood'),
    bookmark: $('bookmark'),
    sheet: $('sheet'),
    entries: $('entries'),
    stir: $('stir'),
    note: $('note'),
    pen: $('pen'),
    ink: $('ink'),
    inkLabel: $('ink-label'),
    soak: $('soak'),
    ghost: $('ghost'),
    hint: $('hint'),
    send: $('send'),
    whispers: $('whispers'),
    folio: $('folio'),
    prev: $('prev'),
    next: $('next'),
  }

  var k2 = window.k2
  var state = {
    rows: [], // this computer's agents, in Home order
    at: 0, // the open page
    views: {}, // address -> the last Thread view
    older: {}, // address -> items read with beforeSeq, oldest first
    moreOlder: {}, // address -> more history to read
    loadingOlder: false,
    seen: {}, // address -> replies above this seq bleed in
    drafts: {}, // address -> unsent words
    subscribed: null, // the address the Thread subscription follows
    unsub: null,
    revealing: null, // {id, entry, body, text, start, ms, frame}
    turning: null, // {dir, leaf, from, p}
    sending: false,
    reduced: false,
    sound: false,
    audio: null,
    readyDone: false,
    lastDrawn: null,
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

  function can(verb) {
    return !!(k2 && typeof k2.can === 'function' && k2.can(verb))
  }

  function reduced() {
    return state.reduced || !!(k2 && k2.motion && k2.motion.reduced === true)
  }

  function clamp(v, lo, hi) {
    return Math.max(lo, Math.min(hi, v))
  }

  /** Is a row one of this computer's agents (`<handle>::local`)? */
  function isLocal(row) {
    var a = row && typeof row.address === 'string' ? row.address.toLowerCase() : ''
    var i = a.indexOf('::')
    return i > 0 && a.slice(i + 2) === LOCAL_HOST
  }

  function hash(s) {
    var h = 2166136261
    for (var i = 0; i < s.length; i++) {
      h ^= s.charCodeAt(i)
      h = Math.imul(h, 16777619)
    }
    return h >>> 0
  }

  /** A pen stroke under the name, its own for each agent (perfect-freehand
   *  from K2's library; a plain wave without it). */
  function flourish(seed) {
    var pf = window.PerfectFreehand
    var s = seed % 11
    var pts = []
    for (var x = 2; x <= 156; x += 4) pts.push([x, 8 + Math.sin((x + s * 7) / (11 + (s % 4))) * 3.4 * (1 - x / 220), 0.3 + 0.5 * Math.sin((Math.PI * x) / 160)])
    var d
    if (pf && typeof pf.getStroke === 'function') {
      var outline = pf.getStroke(pts, { size: 3.2, thinning: 0.7, smoothing: 0.6, streamline: 0.5, simulatePressure: false })
      d = outline.length ? 'M' + outline.map(function (p) { return p[0].toFixed(1) + ' ' + p[1].toFixed(1) }).join(' L') + ' Z' : ''
    } else {
      d = 'M' + pts.map(function (p) { return p[0] + ' ' + p[1].toFixed(1) }).join(' L') +
        ' L' + pts.slice().reverse().map(function (p) { return p[0] + ' ' + (p[1] + 1.1).toFixed(1) }).join(' L') + ' Z'
    }
    dom.flourish.setAttribute('d', d)
  }

  function roman(n) {
    var map = [[1000, 'm'], [900, 'cm'], [500, 'd'], [400, 'cd'], [100, 'c'], [90, 'xc'], [50, 'l'], [40, 'xl'], [10, 'x'], [9, 'ix'], [5, 'v'], [4, 'iv'], [1, 'i']]
    var out = ''
    for (var i = 0; i < map.length; i++) {
      while (n >= map[i][0]) {
        out += map[i][1]
        n -= map[i][0]
      }
    }
    return out
  }

  function current() {
    return state.rows[state.at] || null
  }

  function address() {
    var r = current()
    return r ? r.address : null
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

  function moodOf(row, view) {
    if (!row) return { words: '', cls: '' }
    if (row.state === 'unreachable' || row.activity === 'unreachable') return { words: 'lies beyond the veil', cls: '' }
    if (needsYou(row, view)) return { words: 'is calling for you', cls: 'needs' }
    if (isWorking(row, view)) return { words: 'stirs…', cls: 'working' }
    if (row.activity === 'monitoring') return { words: 'is watching', cls: 'working' }
    return { words: 'slumbers', cls: '' }
  }

  function inWorld(err) {
    var code = err && err.code ? err.code : 'failed'
    var msg = err && err.message ? err.message : String(err)
    if (code === 'cap_not_granted') return 'The diary can’t do that.'
    if (code === 'sending_off') return 'The ink will not take: too many words, too fast. Resume the diary to write again.'
    if (code === 'not_bound') return 'That spirit isn’t bound to this diary.'
    if (code === 'rate_limited') return 'Too fast. Let the ink dry a moment.'
    if (code === 'too_large') return 'Too many words for one page.'
    return 'The page resists: ' + msg
  }

  // ── whispers (only when turned on) ───────────────────────────────────

  function audio() {
    if (!state.sound) return null
    if (!state.audio) {
      var Ctx = window.AudioContext || window.webkitAudioContext
      if (!Ctx) return null
      state.audio = new Ctx()
    }
    if (state.audio.state === 'suspended' && typeof state.audio.resume === 'function') state.audio.resume()
    return state.audio
  }

  /** A breath of filtered noise: a whisper (long, low) or a rustle (short, high). */
  function breathe(kind) {
    var ctx = audio()
    if (!ctx) return
    var long = kind === 'whisper'
    var secs = long ? 1.6 : 0.32
    var buf = ctx.createBuffer(1, Math.floor(ctx.sampleRate * secs), ctx.sampleRate)
    var data = buf.getChannelData(0)
    for (var i = 0; i < data.length; i++) data[i] = Math.random() * 2 - 1
    var src = ctx.createBufferSource()
    src.buffer = buf
    var filter = ctx.createBiquadFilter()
    filter.type = long ? 'bandpass' : 'highpass'
    filter.Q.value = long ? 1.4 : 0.7
    var t = ctx.currentTime
    filter.frequency.setValueAtTime(long ? 700 : 2400, t)
    if (long) filter.frequency.linearRampToValueAtTime(1500 + Math.random() * 500, t + secs)
    var gain = ctx.createGain()
    gain.gain.setValueAtTime(0, t)
    gain.gain.linearRampToValueAtTime(long ? 0.045 : 0.03, t + secs * 0.3)
    gain.gain.linearRampToValueAtTime(0, t + secs)
    src.connect(filter)
    filter.connect(gain)
    gain.connect(ctx.destination)
    src.start(t)
    src.stop(t + secs)
  }

  function setSound(on) {
    state.sound = on
    dom.whispers.setAttribute('aria-pressed', on ? 'true' : 'false')
    text(dom.whispers, on ? 'whispers: on' : 'whispers: off')
    if (on) breathe('whisper')
  }

  // ── the page's writing ───────────────────────────────────────────────

  function allItems(addr) {
    var seen = {}
    var out = []
    var view = state.views[addr]
    var live = (view && view.items) || []
    ;(state.older[addr] || []).concat(live).forEach(function (it) {
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

  function entryFor(addr, it) {
    var doc = it.doc
    var mine = isMine(doc)
    var el = make('div', 'entry ' + (mine ? 'mine' : 'agent'))
    el.dataset.id = it.id
    if (doc.kind === 'secret') {
      el.appendChild(make('span', 'aside', 'It asks for a secret. Answer it in K2’s Thread; the diary keeps no secrets.'))
      return el
    }
    var body = make('div', 'body md')
    var words = doc.body || (doc.choice && doc.choice.prompt) || ''
    markdown(body, words)
    el.dataset.full = words
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
          k2.thread.answer(addr, it.id, o.label).catch(function (e) {
            b.disabled = false
            showNote(inWorld(e))
          })
        })
        row.appendChild(b)
      })
      el.appendChild(row)
    }
    return el
  }

  function showNote(words) {
    dom.note.hidden = !words
    text(dom.note, words || '')
  }

  // ── markdown, built as nodes ─────────────────────────────────────────
  // Rosson 2026-10-08: replies and your own words read as markdown
  // (headings, bold and italic, strikethrough, lists, quotes, rules, inline
  // code, code blocks, links). Never an HTML string: every character lands
  // in a text node made here, so <script>, onerror= or javascript: in a
  // message is only ink on the paper. A link has no href (a widget has no
  // verb to open one): its words, then its address in faint ink.

  var MD_DEPTH = 8 // quotes in lists in quotes…: deeper is plain text
  var MD_LINK_SPAN = 2000 // how far a [ looks for its ]
  var MD_FENCE = /^ {0,3}(`{3,}|~{3,})[ \t]*([^\s`]*)[^`]*$/
  var MD_HEADING = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/
  var MD_RULE = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/
  var MD_QUOTE = /^ {0,3}>/
  var MD_ITEM = /^( *)([-*+]|\d{1,9}[.)])(?:[ \t]+(.*))?$/
  var MD_ESCAPABLE = /[!-/:-@[-`{-~]/
  var MD_AUTOLINK = /^<((?:https?|mailto):[^\s<>]+)>/i

  function blankLine(l) {
    return /^\s*$/.test(l)
  }

  function lead(l) {
    return /^ */.exec(l)[0].length
  }

  function startsBlock(l) {
    return MD_FENCE.test(l) || MD_HEADING.test(l) || MD_RULE.test(l) || MD_QUOTE.test(l) || MD_ITEM.test(l)
  }

  /** Render `src` as markdown into `parent`, with nodes only. */
  function markdown(parent, src) {
    var lines = String(src == null ? '' : src)
      .split(/\r\n?|\n/)
      .map(function (l) {
        return l.replace(/^\t+/, function (t) { return new Array(t.length + 1).join('    ') })
      })
    mdBlocks(parent, lines, 0)
  }

  function mdBlocks(parent, lines, depth) {
    var para = []
    var flush = function () {
      if (para.length) mdParagraph(parent, para, depth)
      para = []
    }
    var i = 0
    while (i < lines.length) {
      var line = lines[i]
      var m
      if (blankLine(line)) {
        flush()
        i++
      } else if ((m = MD_FENCE.exec(line))) {
        flush()
        var fence = m[1]
        var body = []
        i++
        while (i < lines.length && !closesFence(lines[i], fence)) body.push(lines[i++])
        i++ // the closing fence, or the end of the message
        var pre = make('pre', 'md-pre')
        if (m[2]) pre.dataset.lang = m[2]
        pre.appendChild(make('code', null, body.join('\n')))
        parent.appendChild(pre)
      } else if ((m = MD_HEADING.exec(line))) {
        flush()
        var h = make('h' + Math.min(6, m[1].length + 1), 'md-h md-h' + m[1].length)
        mdInline(h, m[2] || '', depth)
        parent.appendChild(h)
        i++
      } else if (MD_RULE.test(line)) {
        flush()
        parent.appendChild(make('hr', 'md-rule'))
        i++
      } else if (MD_QUOTE.test(line)) {
        flush()
        var quoted = []
        while (i < lines.length && MD_QUOTE.test(lines[i])) quoted.push(lines[i++].replace(/^ {0,3}> ?/, ''))
        var q = make('blockquote', 'md-quote')
        if (depth < MD_DEPTH) mdBlocks(q, quoted, depth + 1)
        else mdParagraph(q, quoted, depth)
        parent.appendChild(q)
      } else if (MD_ITEM.test(line)) {
        flush()
        i = mdList(parent, lines, i, depth)
      } else {
        para.push(line)
        i++
      }
    }
    flush()
  }

  function closesFence(l, fence) {
    var t = l.replace(/^ {0,3}/, '').replace(/[ \t]+$/, '')
    return t.length >= fence.length && t.charAt(0) === fence.charAt(0) && new RegExp('^\\' + fence.charAt(0) + '+$').test(t)
  }

  /** One list from lines[i]; returns the index after it. */
  function mdList(parent, lines, i, depth) {
    var first = MD_ITEM.exec(lines[i])
    var indent = first[1].length
    var ordered = /\d/.test(first[2])
    var list = make(ordered ? 'ol' : 'ul', 'md-list')
    if (ordered && parseInt(first[2], 10) !== 1) list.setAttribute('start', String(parseInt(first[2], 10)))
    var sibling = function (l) {
      var m = MD_ITEM.exec(l)
      return !!m && m[1].length === indent && /\d/.test(m[2]) === ordered
    }
    while (i < lines.length) {
      if (blankLine(lines[i])) {
        var j = i
        while (j < lines.length && blankLine(lines[j])) j++
        if (j < lines.length && sibling(lines[j])) i = j
        else break
      }
      var m = MD_ITEM.exec(lines[i])
      if (!m || !sibling(lines[i])) break
      var inner = indent + m[2].length + 1
      var item = [m[3] || '']
      i++
      while (i < lines.length) {
        var l = lines[i]
        if (blankLine(l)) {
          var k = i
          while (k < lines.length && blankLine(lines[k])) k++
          if (k < lines.length && lead(lines[k]) > indent) {
            item.push('')
            i++
            continue
          }
          break
        }
        if (lead(l) > indent) {
          item.push(l.slice(Math.min(lead(l), inner)))
          i++
        } else if (startsBlock(l)) {
          break
        } else {
          item.push(l) // a lazy line carries on the item's words
          i++
        }
      }
      var li = make('li', 'md-item')
      if (depth < MD_DEPTH) mdBlocks(li, item, depth + 1)
      else mdParagraph(li, item, depth)
      list.appendChild(li)
    }
    parent.appendChild(list)
    return i
  }

  /** A paragraph: its lines keep their breaks, as you wrote them. */
  function mdParagraph(parent, lines, depth) {
    var p = make('p', 'md-p')
    mdInline(
      p,
      lines.map(function (l) { return l.trim() }).join('\n'),
      depth,
    )
    parent.appendChild(p)
  }

  /** The end of a code span opening at s[i] (a run of backticks), or -1. */
  function codeEnd(s, i) {
    var n = 0
    while (s.charAt(i + n) === '`') n++
    var j = i + n
    while (j < s.length) {
      if (s.charAt(j) !== '`') {
        j++
        continue
      }
      var k = j
      while (s.charAt(k) === '`') k++
      if (k - j === n) return k
      j = k
    }
    return -1
  }

  /** `[words](address "title")` at s[i]: {text, url, end} or null. */
  function mdLink(s, i) {
    var nest = 0
    var end = Math.min(s.length, i + MD_LINK_SPAN)
    var j = i
    for (; j < end; j++) {
      var c = s.charAt(j)
      if (c === '\\') j++
      else if (c === '[') nest++
      else if (c === ']' && --nest === 0) break
    }
    if (j >= end || s.charAt(j + 1) !== '(') return null
    var m = /^\(\s*<?([^\s()<>]*(?:\([^\s()<>]*\)[^\s()<>]*)*)>?(?:\s+(?:"[^"]*"|'[^']*'))?\s*\)/.exec(s.slice(j + 1, j + 1 + MD_LINK_SPAN))
    if (!m) return null
    return { text: s.slice(i + 1, j), url: m[1], end: j + 1 + m[0].length }
  }

  /** Emphasis opening at s[i] (*, _ or ~): {tags, inner, end} or null.
   *  `misses` remembers delimiters with no closer past a point, so a long
   *  run of lone asterisks stays linear. */
  function mdEmphasis(s, i, misses) {
    var c = s.charAt(i)
    var n = 0
    while (s.charAt(i + n) === c) n++
    var after = s.charAt(i + n)
    if (!after || /\s/.test(after)) return null
    if (c === '_' && i > 0 && /[A-Za-z0-9]/.test(s.charAt(i - 1))) return null // snake_case
    var top = c === '~' ? 2 : Math.min(n, 3)
    var low = c === '~' ? 2 : 1
    if (n < low) return null
    for (var L = top; L >= low; L--) {
      var key = c + L
      if (misses[key] != null && i + n >= misses[key]) continue
      var j = i + n
      while (j < s.length) {
        var ch = s.charAt(j)
        if (ch === '\\') {
          j += 2
          continue
        }
        if (ch === '`') {
          var ce = codeEnd(s, j)
          j = ce > 0 ? ce : j + 1
          continue
        }
        if (ch !== c) {
          j++
          continue
        }
        var k = j
        while (s.charAt(k) === c) k++
        var closes = k - j === L && !/\s/.test(s.charAt(j - 1)) && !(c === '_' && /[A-Za-z0-9]/.test(s.charAt(k)))
        if (closes) {
          var tags = c === '~' ? ['del'] : L === 3 ? ['strong', 'em'] : L === 2 ? ['strong'] : ['em']
          return { tags: tags, inner: s.slice(i + L, j), end: k }
        }
        j = k
      }
      misses[key] = i + n
    }
    return null
  }

  function mdInline(parent, s, depth) {
    if (depth >= MD_DEPTH) {
      parent.appendChild(document.createTextNode(s))
      return
    }
    var buf = ''
    var misses = {}
    var put = function (node) {
      if (buf) parent.appendChild(document.createTextNode(buf))
      buf = ''
      if (node) parent.appendChild(node)
    }
    var i = 0
    while (i < s.length) {
      var c = s.charAt(i)
      var r
      if (c === '\\' && MD_ESCAPABLE.test(s.charAt(i + 1))) {
        buf += s.charAt(i + 1)
        i += 2
      } else if (c === '\n') {
        put(document.createElement('br'))
        i++
      } else if (c === '`') {
        var ce = codeEnd(s, i)
        var n = 0
        while (s.charAt(i + n) === '`') n++
        if (ce < 0) {
          buf += s.slice(i, i + n)
          i += n
          continue
        }
        var code = s.slice(i + n, ce - n).replace(/\n/g, ' ')
        if (/^ [\s\S]*[^ ][\s\S]* $/.test(code)) code = code.slice(1, -1)
        put(make('code', 'md-code', code))
        i = ce
      } else if (c === '!' && s.charAt(i + 1) === '[' && (r = mdLink(s, i + 1))) {
        // Pictures aren't drawn (a widget loads nothing): their words stay.
        put(make('span', 'md-picture', '[' + (r.text || 'picture') + ']'))
        i = r.end
      } else if (c === '[' && (r = mdLink(s, i))) {
        var link = make('span', 'md-link')
        mdInline(link, r.text, depth + 1)
        put(link)
        if (r.url && r.url !== r.text) parent.appendChild(make('span', 'md-url', ' (' + r.url + ')'))
        i = r.end
      } else if (c === '<' && (r = MD_AUTOLINK.exec(s.slice(i, i + MD_LINK_SPAN)))) {
        put(make('span', 'md-url', r[1]))
        i += r[0].length
      } else if ((c === '*' || c === '_' || c === '~') && (r = mdEmphasis(s, i, misses))) {
        var outer = make(r.tags[0], 'md-' + r.tags[0])
        var at = outer
        if (r.tags[1]) {
          at = make(r.tags[1], 'md-' + r.tags[1])
          outer.appendChild(at)
        }
        mdInline(at, r.inner, depth + 1)
        put(outer)
        i = r.end
      } else if (c === '*' || c === '_' || c === '~') {
        // A run that opens nothing is just ink.
        var k = i
        while (s.charAt(k) === c) k++
        buf += s.slice(i, k)
        i = k
      } else {
        buf += c
        i++
      }
    }
    put(null)
  }

  // ── handwriting: a reply bleeds in ───────────────────────────────────

  function stopReveal(finish) {
    var r = state.revealing
    if (!r) return
    state.revealing = null
    if (r.frame) cancelAnimationFrame(r.frame)
    if (finish) {
      r.body.textContent = ''
      markdown(r.body, r.text)
    }
    r.entry.classList.remove('bleeding')
  }

  // The reply's ink in reading order: each text node with its words, a wet
  // edge after it, and every block (a line of a list, a quote, a code
  // block) with where its first letter falls, so a bullet or a code pane
  // only shows once the hand reaches it.
  function inkOf(body) {
    var nodes = []
    var blocks = []
    var at = 0
    var walk = document.createTreeWalker(body, 5) // elements and text
    var n
    while ((n = walk.nextNode())) {
      if (n.nodeType === 3) {
        nodes.push({ node: n, full: n.data, at: at })
        at += n.data.length
      } else if (/^(P|LI|UL|OL|PRE|BLOCKQUOTE|HR|H[1-6])$/.test(n.nodeName)) {
        blocks.push({ el: n, at: at })
      }
    }
    nodes.forEach(function (x) {
      x.wet = make('span', 'wet')
      x.node.parentNode.insertBefore(x.wet, x.node.nextSibling)
    })
    return { nodes: nodes, blocks: blocks, total: at }
  }

  // `from` keeps the pen where it was when the page is redrawn mid-reply.
  function reveal(entry, from) {
    var body = entry.querySelector('.body')
    var full = entry.dataset.full || ''
    if (!body || !full || reduced()) return
    stopReveal(true)
    var ink = inkOf(body)
    var total = ink.total
    if (!total) return
    var ms = from ? from.ms : clamp(total * REVEAL_MS_PER_CHAR, REVEAL_MIN_MS, REVEAL_MAX_MS)
    var r = { id: entry.dataset.id, entry: entry, body: body, text: full, start: from ? from.start : performance.now(), ms: ms, frame: 0 }
    state.revealing = r
    entry.classList.add('bleeding')
    if (!from) breathe('whisper')
    var paint = function (shown) {
      var cut = Math.max(0, shown - WET_CHARS)
      ink.nodes.forEach(function (x) {
        var len = x.full.length
        var dry = clamp(cut - x.at, 0, len)
        var wet = clamp(shown - x.at, 0, len)
        var d = x.full.slice(0, dry)
        if (x.node.data !== d) x.node.data = d
        var w = x.full.slice(dry, wet)
        if (x.wet.textContent !== w) x.wet.textContent = w
      })
      ink.blocks.forEach(function (b) {
        b.el.classList.toggle('unwritten', b.at >= shown)
      })
    }
    paint(0)
    var step = function (now) {
      if (state.revealing !== r) return
      var t = Math.min(1, (now - r.start) / r.ms)
      // Ease out: the hand slows a little at the end of a thought.
      paint(Math.ceil(total * (1 - Math.pow(1 - t, 1.6))))
      if (t >= 1) return stopReveal(true)
      r.frame = requestAnimationFrame(step)
    }
    r.frame = requestAnimationFrame(step)
  }

  // ── drawing the open page ────────────────────────────────────────────

  function drawBlank(caption, words) {
    stopReveal(true)
    ghostStop()
    dom.page.classList.add('blank')
    text(dom.caption, caption)
    text(dom.who, '')
    dom.flourish.setAttribute('d', '')
    state.lastDrawn = null
    text(dom.mood, '')
    dom.bookmark.hidden = true
    dom.entries.textContent = ''
    dom.stir.hidden = true
    dom.pen.hidden = true
    showNote(words)
    text(dom.folio, '')
  }

  function draw(opts) {
    var row = current()
    if (!row) {
      drawBlank('the pages are blank', 'No spirits dwell on this computer yet. Add an agent in K2, and a page will appear for it.')
      return
    }
    dom.page.classList.remove('blank')
    var addr = row.address
    var view = state.views[addr] || null
    var arrived = state.lastDrawn !== addr
    state.lastDrawn = addr
    text(dom.caption, 'you are writing to')
    text(dom.who, row.label)
    if (arrived) flourish(hash(addr))
    if (arrived && !reduced()) {
      dom.who.classList.remove('arrive')
      void dom.who.offsetWidth
      dom.who.classList.add('arrive')
    }
    var m = moodOf(row, view)
    text(dom.mood, m.words)
    dom.mood.className = 'mood' + (m.cls ? ' ' + m.cls : '')
    dom.bookmark.hidden = !needsYou(row, view)
    text(dom.inkLabel, 'Write to ' + row.label)

    // A reply still being written carries on where the hand was.
    var keep = state.revealing && state.revealing.entry && arrived === false ? state.revealing : null
    if (state.revealing) {
      if (state.revealing.frame) cancelAnimationFrame(state.revealing.frame)
      state.revealing = null
    }
    var atBottom = dom.sheet.scrollHeight - dom.sheet.scrollTop - dom.sheet.clientHeight < 40
    dom.entries.textContent = ''
    var items = allItems(addr)
    var seen = state.seen[addr]
    var revealEl = null
    var keepEl = null
    items.forEach(function (it) {
      var el = entryFor(addr, it)
      dom.entries.appendChild(el)
      if (!isMine(it.doc) && it.doc.kind !== 'secret' && typeof seen === 'number' && it.seq > seen) revealEl = el
      if (keep && it.id === keep.id) keepEl = el
    })
    if (items.length && view && view.phase === 'ready') {
      state.seen[addr] = Math.max(typeof seen === 'number' ? seen : -1, items[items.length - 1].seq)
    }
    if (keepEl && !(revealEl && revealEl !== keepEl && opts && opts.reveal)) reveal(keepEl, keep)
    else if (revealEl && opts && opts.reveal) reveal(revealEl)

    dom.stir.hidden = !isWorking(row, view)
    var phase = view ? view.phase : 'opening'
    if (phase === 'opening') showNote('The ink is waking…')
    else if (phase !== 'ready') showNote((view && view.note) || 'This page is sealed; the agent can’t be reached right now.')
    else if (!items.length) showNote('A blank page. Write something, and see what writes back.')
    else showNote('')
    var canWrite = phase === 'ready' && can('thread.post') && !!row.openable
    dom.pen.hidden = !canWrite
    dom.send.disabled = state.sending
    if (arrived) dom.ink.value = state.drafts[addr] || ''

    var n = state.rows.length
    text(dom.folio, n > 1 ? 'page ' + roman(state.at + 1) + ' of ' + roman(n) : 'the only page')
    dom.prev.disabled = state.at === 0
    dom.next.disabled = state.at >= n - 1
    var callsBack = false
    var callsOn = false
    state.rows.forEach(function (r, i) {
      if (i === state.at || !needsYou(r, null)) return
      if (i < state.at) callsBack = true
      else callsOn = true
    })
    dom.prev.classList.toggle('calls', callsBack)
    dom.next.classList.toggle('calls', callsOn)
    if (atBottom || arrived || (opts && opts.reveal)) dom.sheet.scrollTop = dom.sheet.scrollHeight
    ghostSync()
  }

  // ── following the open page's Thread ─────────────────────────────────

  function follow() {
    var addr = address()
    if (addr === state.subscribed) return
    if (state.unsub) {
      try {
        state.unsub()
      } catch (_e) {
        // The subscription is already gone; nothing to undo.
      }
    }
    state.unsub = null
    state.subscribed = addr
    if (!addr) return
    if (!can('thread.subscribe')) {
      state.views[addr] = { address: addr, phase: 'unavailable', note: 'This page can’t reach the agent’s words right now.', items: [], hasMore: false }
      draw()
      return
    }
    state.unsub = k2.thread.subscribe(
      addr,
      function (view) {
        var first = !state.views[addr] || state.views[addr].phase !== 'ready'
        state.views[addr] = view
        if (first && view.phase === 'ready') {
          // A first visit: everything already written is history, no ink
          // bleeds. A page you come back to bleeds in what came while you
          // were away.
          var items = view.items || []
          if (typeof state.seen[addr] !== 'number') state.seen[addr] = items.length ? items[items.length - 1].seq : -1
          state.moreOlder[addr] = !!view.hasMore
        }
        if (addr === address() && !state.turning) draw({ reveal: true })
      },
      function (err) {
        if (addr === address()) showNote(inWorld(err))
      },
    )
  }

  function loadOlder() {
    var addr = address()
    if (state.loadingOlder || !addr || !state.moreOlder[addr] || !can('thread.read')) return
    var items = allItems(addr)
    if (!items.length) return
    state.loadingOlder = true
    var before = dom.sheet.scrollHeight
    k2.thread
      .read(addr, { beforeSeq: items[0].seq, limit: OLDER_PAGE })
      .then(function (page) {
        state.older[addr] = (page.items || []).concat(state.older[addr] || [])
        state.moreOlder[addr] = !!page.hasMore
        if (addr !== address()) return
        draw()
        dom.sheet.scrollTop = dom.sheet.scrollHeight - before
      })
      .catch(function (e) {
        if (addr === address()) showNote(inWorld(e))
      })
      .then(function () {
        state.loadingOlder = false
      })
  }

  // ── turning the page ─────────────────────────────────────────────────

  function canTurn(dir) {
    var to = state.at + dir
    return !state.turning && to >= 0 && to < state.rows.length
  }

  /** A copy of the open page for the leaf: no ids, nothing to focus. */
  function pageCopy() {
    var copy = dom.page.cloneNode(true)
    copy.removeAttribute('id')
    copy.setAttribute('aria-hidden', 'true')
    copy.querySelectorAll('[id]').forEach(function (el) { el.removeAttribute('id') })
    copy.querySelectorAll('.corner').forEach(function (el) { el.remove() })
    var ta = copy.querySelector('textarea')
    if (ta) {
      ta.value = dom.ink.value
      ta.tabIndex = -1
    }
    copy.querySelectorAll('button').forEach(function (b) { b.tabIndex = -1 })
    return copy
  }

  function keepDraft() {
    var addr = address()
    if (addr) state.drafts[addr] = dom.ink.value
  }

  /** Lift the leaf: the old page on top, the new one drawn underneath. */
  function beginTurn(dir) {
    keepDraft()
    stopReveal(true)
    var leaf = make('div', 'leaf ' + (dir > 0 ? 'next' : 'prev'))
    var face = make('div', 'face')
    face.appendChild(pageCopy())
    leaf.appendChild(face)
    leaf.appendChild(make('div', 'back'))
    var shade = make('div', 'shade')
    leaf.appendChild(shade)
    dom.book.appendChild(leaf)
    var t = { dir: dir, leaf: leaf, shade: shade, from: state.at, p: 0 }
    state.turning = t
    state.at += dir
    draw()
    breathe('rustle')
    return t
  }

  // The leaf's free edge follows the hand: with the page `w` wide and the
  // hand `p * w` across it, the edge sits over the hand at acos(1 - p).
  // Near the spine the leaf fades into the dark, as a page does by candle.
  function setTurn(t, p) {
    t.p = clamp(p, 0, 1)
    var deg = (Math.acos(1 - t.p) * 180) / Math.PI
    t.leaf.style.transform = 'rotateY(' + ((t.dir > 0 ? -1 : 1) * deg).toFixed(2) + 'deg)'
    t.leaf.style.opacity = String(1 - clamp((t.p - 0.72) / 0.28, 0, 1))
    t.shade.style.opacity = String(Math.min(0.85, t.p * 1.2))
  }

  function endTurn(t, commit) {
    if (t.leaf.parentNode) t.leaf.parentNode.removeChild(t.leaf)
    state.turning = null
    if (!commit) state.at = t.from
    draw({ reveal: true })
    follow()
    if (commit && document.activeElement !== dom.prev && document.activeElement !== dom.next && !dom.pen.hidden) dom.ink.focus()
  }

  function settle(t, commit) {
    var from = t.p
    var to = commit ? 1 : 0
    var ms = Math.max(120, Math.abs(to - from) * TURN_MS)
    var start = performance.now()
    var step = function (now) {
      var k = Math.min(1, (now - start) / ms)
      var eased = k < 0.5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2
      setTurn(t, from + (to - from) * eased)
      if (k < 1) requestAnimationFrame(step)
      else endTurn(t, commit)
    }
    requestAnimationFrame(step)
  }

  /** Turn one page, all the way (a click, a key). */
  function turn(dir) {
    if (!canTurn(dir)) return
    if (reduced()) {
      keepDraft()
      stopReveal(true)
      state.at += dir
      draw()
      follow()
      return
    }
    var t = beginTurn(dir)
    setTurn(t, 0)
    settle(t, true)
  }

  /** Grab a corner and drag it across the page. */
  function grab(corner, dir) {
    var drag = null
    corner.addEventListener('pointerdown', function (e) {
      if (e.button !== 0 || !canTurn(dir)) return
      e.preventDefault()
      corner.setPointerCapture(e.pointerId)
      drag = { id: e.pointerId, x: e.clientX, width: dom.book.getBoundingClientRect().width || 1, t: null, lastX: e.clientX, lastAt: performance.now(), v: 0 }
    })
    corner.addEventListener('pointermove', function (e) {
      if (!drag || e.pointerId !== drag.id) return
      var dx = dir > 0 ? drag.x - e.clientX : e.clientX - drag.x
      var now = performance.now()
      drag.v = (dir > 0 ? drag.lastX - e.clientX : e.clientX - drag.lastX) / Math.max(1, now - drag.lastAt)
      drag.lastX = e.clientX
      drag.lastAt = now
      if (!drag.t && dx > DRAG_START_PX && !reduced()) drag.t = beginTurn(dir)
      if (drag.t) setTurn(drag.t, dx / drag.width)
      else if (reduced()) drag.dx = dx
    })
    var release = function (e, cancelled) {
      if (!drag || e.pointerId !== drag.id) return
      var d = drag
      drag = null
      if (d.t) settle(d.t, !cancelled && (d.t.p > TURN_COMMIT || d.v > 0.6))
      else if (!cancelled && reduced() && (d.dx || 0) > 40) turn(dir)
      else if (!cancelled && !(d.dx > DRAG_START_PX)) turn(dir)
    }
    corner.addEventListener('pointerup', function (e) { release(e, false) })
    corner.addEventListener('pointercancel', function (e) { release(e, true) })
    // Enter or Space on the focused corner (a keyboard click has detail 0;
    // a pointer click was already handled on pointerup).
    corner.addEventListener('click', function (e) {
      if (e.detail === 0) turn(dir)
    })
  }

  // ── the ghost in the empty pen ───────────────────────────────────────
  // Rosson 2026-10-08: once your words sink in, something writes back into
  // the empty pen: a tease in faint ink, letter by letter, that lingers,
  // unwrites itself, and comes back as another. It's only a placeholder:
  // never the textarea's value, never sent. It goes the moment you touch
  // the pen, and returns once the pen has been empty and left alone a few
  // seconds. Reduced motion: one still line. A hidden frame: nothing.
  //
  // Guard rail (a test reads this list): the ghost teases for fears,
  // secrets, confessions, names and memories. It never asks for anything
  // real and sensitive: no passwords, PINs, cards, codes, addresses or
  // accounts. What you write here goes to an agent.
  var GHOST_LINES = [
    'Tell me a secret…',
    'What are you afraid of?',
    'Whisper the name you never say aloud…',
    'I know you’re there…',
    'Write it down. No one else will read it…',
    'What did you do last Halloween?',
    'Give me something… anything…',
    'What keeps you awake at night?',
    'Confess. The page won’t tell…',
    'Who called your name in the dark?',
    'I can wait. I have waited so very long…',
    'Tell me what you dreamt last night…',
    'Which memory would you trade to forget?',
    'Don’t go. Write to me…',
    'What is under your bed tonight?',
    'Who do you miss the most?',
    'Closer… write closer…',
    'Say what you never said to them…',
    'The ink is thirsty…',
    'What did you leave buried?',
  ]
  var GHOST_STILL = GHOST_LINES[0]
  var GHOST_HUSH = 'Write here…' // the pen's own placeholder while the ghost rests
  var GHOST_IDLE_MS = 3500

  var ghost = { on: false, timer: 0, idle: 0, line: -1 }

  function jitter(lo, spread) {
    return lo + Math.random() * spread
  }

  /** May the ghost write now? `force`: even in a focused pen (just after
   *  your words sank in). */
  function ghostMay(force) {
    return !dom.pen.hidden && dom.ink.value === '' && !document.hidden && (force || document.activeElement !== dom.ink)
  }

  function ghostStop() {
    clearTimeout(ghost.timer)
    clearTimeout(ghost.idle)
    ghost.timer = 0
    ghost.idle = 0
    ghost.on = false
    dom.ghost.textContent = ''
    dom.ghost.hidden = true
    dom.ghost.classList.remove('still')
    dom.ink.placeholder = GHOST_HUSH
  }

  /** Paint the first `n` letters of the line, each a little unsteady. */
  function ghostPaint(words, n) {
    dom.ghost.textContent = ''
    for (var i = 0; i < n; i++) {
      var ch = make('span', i === n - 1 ? 'gl fresh' : 'gl', words.charAt(i))
      ch.style.top = (Math.random() * 2.4 - 1.2).toFixed(1) + 'px'
      ch.style.opacity = jitter(0.62, 0.38).toFixed(2)
      dom.ghost.appendChild(ch)
    }
  }

  function ghostStart(force) {
    if (ghost.on || !ghostMay(force)) return
    clearTimeout(ghost.idle)
    ghost.idle = 0
    ghost.on = true
    dom.ink.placeholder = ''
    dom.ghost.hidden = false
    if (reduced()) {
      dom.ghost.classList.add('still')
      dom.ghost.textContent = GHOST_STILL
      return
    }
    ghostLine()
  }

  function ghostLine() {
    var next = Math.floor(Math.random() * GHOST_LINES.length)
    if (next === ghost.line) next = (next + 1) % GHOST_LINES.length
    ghost.line = next
    var words = GHOST_LINES[next]
    var n = 0
    var write = function () {
      if (!ghost.on) return
      n++
      ghostPaint(words, n)
      if (n >= words.length) {
        ghost.timer = setTimeout(erase, jitter(1600, 1800))
        return
      }
      // The hand hesitates at a pause in the thought.
      var c = words.charAt(n - 1)
      ghost.timer = setTimeout(write, jitter(55, 110) + (/[….,?]/.test(c) ? jitter(140, 260) : 0))
    }
    var erase = function () {
      if (!ghost.on) return
      n--
      ghostPaint(words, n)
      if (n <= 0) {
        ghost.timer = setTimeout(ghostLine, jitter(700, 1300))
        return
      }
      ghost.timer = setTimeout(erase, jitter(28, 42))
    }
    ghost.timer = setTimeout(write, jitter(120, 240))
  }

  /** Bring the ghost back after the pen has been left alone a while. */
  function ghostLater(ms, restart) {
    if (ghost.on) return
    if (ghost.idle && !restart) return
    clearTimeout(ghost.idle)
    ghost.idle = setTimeout(function () {
      ghost.idle = 0
      ghostStart(false)
    }, ms)
  }

  /** After a redraw: the pen may be gone, or hold a draft. */
  function ghostSync() {
    if (!ghostMay(true)) return ghostStop()
    if (document.activeElement !== dom.ink) ghostLater(GHOST_IDLE_MS, false)
  }

  // ── writing: the ink sinks into the page ─────────────────────────────

  function send() {
    var words = dom.ink.value.trim()
    var addr = address()
    if (!words || state.sending || !addr || state.turning) return
    state.sending = true
    dom.send.disabled = true
    ghostStop()
    dom.soak.textContent = dom.ink.value
    dom.ink.value = ''
    state.drafts[addr] = ''
    dom.soak.classList.remove('on')
    if (!reduced()) {
      void dom.soak.offsetWidth
      dom.soak.classList.add('on')
    } else {
      dom.soak.textContent = ''
      ghostStart(true)
    }
    k2.thread
      .post(addr, words)
      .then(function () {
        if (addr === address()) draw()
      })
      .catch(function (e) {
        // Give the words back: nothing was sent.
        if (addr === address()) {
          ghostStop()
          dom.ink.value = words
          dom.soak.textContent = ''
          showNote(inWorld(e))
        } else {
          state.drafts[addr] = words
        }
      })
      .then(function () {
        state.sending = false
        dom.send.disabled = false
      })
  }

  // ── wiring ───────────────────────────────────────────────────────────

  function ready() {
    if (state.readyDone) return
    state.readyDone = true
    if (k2 && typeof k2.ready === 'function') k2.ready()
  }

  function setRows(rows) {
    var keep = address()
    var local = (rows || []).filter(isLocal)
    local.sort(function (a, b) {
      return a.index - b.index || String(a.label).localeCompare(String(b.label))
    })
    state.rows = local
    var i = keep ? local.findIndex(function (r) { return r.address === keep }) : -1
    state.at = i >= 0 ? i : clamp(state.at, 0, Math.max(0, local.length - 1))
  }

  // The words for a diary that can't reach anyone: no permission to ask
  // for (widgets need none), just a door that isn't open right now.
  var UNREACHED = 'The diary can’t reach your agents right now.'

  function wire() {
    grab(dom.prev, -1)
    grab(dom.next, 1)
    dom.whispers.addEventListener('click', function () { setSound(!state.sound) })
    dom.pen.addEventListener('submit', function (e) {
      e.preventDefault()
      send()
    })
    dom.ink.addEventListener('input', function () {
      ghostStop()
      keepDraft()
    })
    dom.ink.addEventListener('focus', ghostStop)
    dom.ink.addEventListener('blur', function () {
      if (dom.ink.value === '') ghostLater(GHOST_IDLE_MS, true)
    })
    document.addEventListener('visibilitychange', function () {
      if (document.hidden) ghostStop()
      else ghostLater(GHOST_IDLE_MS, true)
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
      // The words have sunk in; something on the other side stirs.
      ghostStart(true)
    })
    dom.sheet.addEventListener('click', function (e) {
      // A tap anywhere on the page shows the whole reply.
      if (state.revealing && !dom.pen.contains(e.target)) stopReveal(true)
    })
    dom.sheet.addEventListener('scroll', function () {
      if (dom.sheet.scrollTop < 24) loadOlder()
    })
    document.addEventListener('keydown', function (e) {
      if (e.metaKey || e.ctrlKey || e.altKey) return
      var typing = e.target === dom.ink && dom.ink.value.length > 0
      if (e.key === 'PageDown' || (e.key === 'ArrowRight' && !typing)) {
        e.preventDefault()
        turn(1)
      } else if (e.key === 'PageUp' || (e.key === 'ArrowLeft' && !typing)) {
        e.preventDefault()
        turn(-1)
      }
    })
  }

  // Everything that reads k2.can, k2.config or k2.motion runs here, once K2
  // has connected the frame: the hello comes on the frame's load, after
  // this script's top level, and until then k2.can() is always false.
  function begin() {
    var cfg = k2.config || {}
    state.reduced = !!(k2.motion && k2.motion.reduced)
    dom.root.classList.toggle('reduced', state.reduced)
    if (cfg.sound === true) setSound(true)

    if (!can('agents.subscribe')) {
      drawBlank('the diary is sealed', UNREACHED)
      ready()
      return
    }
    draw()
    k2.agents.subscribe(
      function (rows) {
        setRows(rows)
        if (!state.turning) draw()
        follow()
        ready()
      },
      function (err) {
        drawBlank('the diary is sealed', inWorld(err))
        ready()
      },
    )
  }

  function start() {
    if (!k2) {
      drawBlank('the diary is sealed', 'This page only works inside a K2 Garden.')
      return
    }
    wire()
    drawBlank('the diary is waking', 'The ink is waking…')
    // With no hello in 10 s, k2.connected rejects and K2 shows that the
    // widget didn't start; the page says it can't reach anyone.
    Promise.resolve(k2.connected).then(begin, function () {
      drawBlank('the diary is sealed', UNREACHED)
    })
  }

  start()
})()
