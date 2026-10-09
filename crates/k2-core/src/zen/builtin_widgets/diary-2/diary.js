// k2:diary@2 — the haunted Diary (Rosson 2026-10-08; prd-zen-user-widgets-v2
// UWB24). One aged page per agent on THIS computer, and nothing else to
// choose: grab a page's corner and turn it (drag it, click it, or use the
// arrow keys) to write to another agent. What you write sinks into the
// paper; the reply arrives whole (Thread doesn't stream) and bleeds back in
// handwriting at a natural pace, capped near 6 s, and a tap shows it all.
// While the agent works, a pen scratches; when it needs you, a bookmark lifts
// and the corner toward its page smoulders. Reduced motion stills every
// effect. No buttons on the page: Enter seals your words.
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
// Ink, paper and sound (Rosson 2026-10-08), each from K2's library and each
// optional (a missing library leaves the Diary as it was):
//   - perfect-freehand: the flourish under a name and the odd blot under a
//     reply are filled pen outlines with pressure and tapered ends (the
//     scratching pen keeps its own wild stroked lines: Rosson 2026-10-08,
//     "the old scribbles were better and more scary/chaotic");
//   - PixiJS: one WebGL canvas UNDER the words (paper fibre and grain, the
//     candle warming the page in step with the CSS flicker, ink bleeding
//     into the fibres under freshly written words), drawn on demand (at
//     most 30 fps while ink is wet; at rest only when the flame flickers),
//     stopped while hidden, one still frame under reduced motion;
//   - Tone.js: a quiet haunted room (a low drone, distant detuned bells,
//     creaks, wind) made in code, starting when you arrive on this Garden
//     and fading out when you leave; the speaker at the top left, or M,
//     mutes it.
//
// Rules this file keeps (the k2-zen skill's): agent text only ever goes
// through textContent; no inline handlers; no network or storage.
(function () {
  'use strict'

  var LOCAL_HOST = 'local'
  var SVG_NS = 'http://www.w3.org/2000/svg'
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
    scribble: $('scribble'),
    ghosts: $('ghosts'),
    moonbeam: $('moonbeam'),
    note: $('note'),
    pen: $('pen'),
    ink: $('ink'),
    inkLabel: $('ink-label'),
    soak: $('soak'),
    ghost: $('ghost'),
    folio: $('folio'),
    prev: $('prev'),
    next: $('next'),
    glow: $('glow'),
    candle: $('candle'),
    hush: $('hush'),
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

  // ── ink: perfect-freehand outlines ───────────────────────────────────
  // A pen line is a filled outline, not a stroked line: perfect-freehand
  // (K2's library) widens it where the hand slows and presses, thins it
  // where it hurries, and tapers its ends. `simulatePressure` reads the
  // hand's speed from the spacing of the points.

  function n1(v) {
    return (Math.round(v * 10) / 10).toFixed(1)
  }

  /** The outline polygon of a pen line, or null without the library. */
  function inkOutline(points, o) {
    var pf = window.PerfectFreehand
    if (!pf || typeof pf.getStroke !== 'function' || points.length < 2) return null
    var out = pf.getStroke(points, {
      size: o.size,
      thinning: o.thinning,
      smoothing: o.smoothing == null ? 0.55 : o.smoothing,
      streamline: o.streamline == null ? 0.35 : o.streamline,
      simulatePressure: o.pressure !== false,
      start: { taper: o.taperStart || 0, cap: true },
      end: { taper: o.taperEnd || 0, cap: true },
      last: true,
    })
    return out && out.length > 3 ? out : null
  }

  /** An outline as a closed SVG path: quadratic curves through the
   *  midpoints (perfect-freehand's own recipe). `box` [x0, y0, x1, y1]
   *  keeps every point inside it. */
  function outlineD(outline, box) {
    var pts = box
      ? outline.map(function (p) { return [clamp(p[0], box[0], box[2]), clamp(p[1], box[1], box[3])] })
      : outline
    var a = pts[0]
    var b = pts[1]
    var c = pts[2]
    var d = 'M' + n1(a[0]) + ' ' + n1(a[1]) + ' Q' + n1(b[0]) + ' ' + n1(b[1]) + ' ' + n1((b[0] + c[0]) / 2) + ' ' + n1((b[1] + c[1]) / 2) + ' T'
    for (var i = 2; i < pts.length - 1; i++) d += n1((pts[i][0] + pts[i + 1][0]) / 2) + ' ' + n1((pts[i][1] + pts[i + 1][1]) / 2) + ' '
    return d + 'Z'
  }

  /** A seeded die (mulberry32): the same blot for the same message. */
  function dice(seed) {
    var a = seed >>> 0
    return function () {
      a = (a + 0x6d2b79f5) | 0
      var t = Math.imul(a ^ (a >>> 15), 1 | a)
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296
    }
  }

  /** A pen stroke under the name, its own for each agent (a plain wave
   *  without the library). */
  function flourish(seed) {
    var s = seed % 11
    var pts = []
    for (var x = 4; x <= 154; x += 4) pts.push([x, 8 + Math.sin((x + s * 7) / (11 + (s % 4))) * 3.4 * (1 - x / 220), 0.3 + 0.5 * Math.sin((Math.PI * x) / 160)])
    var outline = inkOutline(pts, { size: 3.2, thinning: 0.7, smoothing: 0.6, streamline: 0.5, pressure: false, taperStart: 10, taperEnd: 40 })
    var d
    if (outline) {
      d = outlineD(outline, [0.5, 0.5, 159.5, 15.5])
    } else {
      d = 'M' + pts.map(function (p) { return p[0] + ' ' + p[1].toFixed(1) }).join(' L') +
        ' L' + pts.slice().reverse().map(function (p) { return p[0] + ' ' + (p[1] + 1.1).toFixed(1) }).join(' L') + ' Z'
    }
    dom.flourish.setAttribute('d', d)
  }

  /** Where the pen rested under a reply: a pressed blob with a short
   *  dragged tail and a drop or two, drawn with the same pen. Only some
   *  replies get one (by message id, so it never moves). */
  var BLOT_W = 40
  var BLOT_H = 28

  function blot(id) {
    var h = hash('blot:' + id)
    if (h % 4 !== 1 || !window.PerfectFreehand) return null
    var roll = dice(h)
    var svg = document.createElementNS(SVG_NS, 'svg')
    svg.setAttribute('class', 'blot')
    svg.setAttribute('viewBox', '0 0 ' + BLOT_W + ' ' + BLOT_H)
    svg.setAttribute('aria-hidden', 'true')
    svg.setAttribute('focusable', 'false')
    var cx = 15 + roll() * 6
    var cy = 12 + roll() * 4
    var pts = []
    var lobes = 2 + Math.floor(roll() * 3)
    var phase = roll() * Math.PI * 2
    for (var t = 0; t < Math.PI * 3.4; t += 0.4) {
      var r = (0.6 + t * 0.36) * (1 + 0.28 * Math.sin(t * lobes + phase))
      pts.push([cx + Math.cos(t) * r, cy + Math.sin(t) * r * 0.75, 1])
    }
    var ang = roll() * Math.PI * 2
    var tail = 4 + roll() * 6
    for (var k = 1; k <= 4; k++) pts.push([cx + Math.cos(ang) * tail * (k / 4), cy + Math.sin(ang) * tail * (k / 4) * 0.7, 1 - k / 5])
    var strokes = [{ pts: pts, o: { size: 6.5, thinning: 0.55, pressure: false, taperEnd: 6 } }]
    var drops = 1 + Math.floor(roll() * 3)
    for (var j = 0; j < drops; j++) {
      var da = roll() * Math.PI * 2
      var dd = 9 + roll() * 6
      var dx = cx + Math.cos(da) * dd
      var dy = cy + Math.sin(da) * dd * 0.6
      strokes.push({ pts: [[dx, dy, 1], [dx + 0.6, dy + 0.3, 1]], o: { size: 1.6 + roll() * 1.8, thinning: 0.2, pressure: false } })
    }
    strokes.forEach(function (st) {
      var outline = inkOutline(st.pts, st.o)
      if (!outline) return
      var p = document.createElementNS(SVG_NS, 'path')
      p.setAttribute('d', outlineD(outline, [0.5, 0.5, BLOT_W - 0.5, BLOT_H - 0.5]))
      svg.appendChild(p)
    })
    return svg
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
    if (!mine) {
      var b = blot(it.id)
      if (b) el.appendChild(b)
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
    stirSync()
    glideOn()
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
    stirSync() // the hand that scratched now writes
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
      fxWet(now, ink) // read the wet words' box before this frame writes
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
    scribeStop()
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
    dom.ink.setAttribute('aria-label', 'Write to ' + row.label + ' in the diary')

    // A reply still being written carries on where the hand was.
    var keep = state.revealing && state.revealing.entry && arrived === false ? state.revealing : null
    if (state.revealing) {
      if (state.revealing.frame) cancelAnimationFrame(state.revealing.frame)
      state.revealing = null
    }
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

    stirSync()
    var phase = view ? view.phase : 'opening'
    if (phase === 'opening') showNote('The ink is waking…')
    else if (phase !== 'ready') showNote((view && view.note) || 'This page is sealed; the agent can’t be reached right now.')
    else if (!items.length) showNote('A blank page. Write something, and see what writes back.')
    else showNote('')
    var canWrite = phase === 'ready' && can('thread.post') && !!row.openable
    dom.pen.hidden = !canWrite
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
    // A page opened, or its history just arrived: start at the bottom.
    if (arrived || (opts && opts.snap)) glideSnap()
    else glideOn()
    ghostSync()
  }

  // ── the page glides up as the hand writes ────────────────────────────
  // Rosson 2026-10-08: the writing point stays in view, the page sliding up
  // continuously while a reply bleeds in or new lines arrive, never in
  // steps. One loop per frame: read the sheet's layout once, then write
  // scrollTop once, easing toward the bottom (exponential, so it never
  // overshoots). Scroll up by hand and it lets go; come back to the bottom
  // and it follows again. Reduced motion: it snaps.

  var GLIDE_TAU_MS = 110 // how quickly the page catches up
  var GLIDE_SLACK = 40 // this close to the bottom counts as "at the bottom"

  var glide = { pinned: true, frame: 0, last: 0, wrote: null }

  function glideBottom() {
    return Math.max(0, dom.sheet.scrollHeight - dom.sheet.clientHeight)
  }

  function glideWrite(top) {
    glide.wrote = top
    fx.scroll = top
    dom.sheet.scrollTop = top
  }

  /** Jump to the bottom at once (a new page, reduced motion). */
  function glideSnap() {
    glide.pinned = true
    if (glide.frame) cancelAnimationFrame(glide.frame)
    glide.frame = 0
    glideWrite(glideBottom())
  }

  /** Follow the writing, if the reader is following. */
  function glideOn() {
    if (!glide.pinned) return
    if (reduced()) return glideWrite(glideBottom())
    if (glide.frame) return
    glide.last = 0
    glide.frame = requestAnimationFrame(glideStep)
  }

  function glideStep(now) {
    glide.frame = 0
    if (!glide.pinned) return
    // Read once…
    var target = glideBottom()
    var top = dom.sheet.scrollTop
    var dt = glide.last ? Math.min(64, Math.max(0, now - glide.last)) : 16
    glide.last = now
    var gap = target - top
    // …then write once.
    if (Math.abs(gap) <= 0.5) {
      if (gap !== 0) glideWrite(target)
      if (!state.revealing) return // nothing more is coming: rest
    } else {
      glideWrite(top + gap * (1 - Math.exp(-dt / GLIDE_TAU_MS)))
    }
    glide.frame = requestAnimationFrame(glideStep)
  }

  /** A scroll we didn't write is the reader's hand: up lets go, back at
   *  the bottom takes hold again. */
  function glideScrolled() {
    var top = dom.sheet.scrollTop
    if (glide.wrote != null && Math.abs(top - glide.wrote) <= 2) return
    glide.wrote = null
    var atBottom = glideBottom() - top < GLIDE_SLACK
    if (atBottom && !glide.pinned) {
      glide.pinned = true
      glideOn()
    } else if (!atBottom && glide.pinned) {
      glide.pinned = false
      if (glide.frame) cancelAnimationFrame(glide.frame)
      glide.frame = 0
    }
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
        if (addr === address() && !state.turning) draw({ reveal: true, snap: first && view.phase === 'ready' })
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
        glideWrite(dom.sheet.scrollHeight - before)
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
    copy.querySelectorAll('.corner, canvas').forEach(function (el) { el.remove() })
    var ta = copy.querySelector('textarea')
    if (ta) {
      ta.value = dom.ink.value
      ta.tabIndex = -1
    }
    copy.querySelectorAll('button').forEach(function (b) { b.tabIndex = -1 })
    return copy
  }

  /** Focus may scroll a clipped box (the desk, the book, the page) to show
   *  the pen, sliding the page's writing out of sight; only the sheet
   *  scrolls. */
  function unslide() {
    ;[dom.desk, dom.book, dom.page].forEach(function (el) {
      if (el.scrollTop) el.scrollTop = 0
      if (el.scrollLeft) el.scrollLeft = 0
    })
  }

  function keepDraft() {
    var addr = address()
    if (addr) state.drafts[addr] = dom.ink.value
  }

  /** Lift the leaf: the old page on top, the new one drawn underneath. */
  function beginTurn(dir) {
    keepDraft()
    stopReveal(true)
    fxClear()
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
      fxClear()
      state.at += dir
      draw()
      follow()
      return
    }
    var t = beginTurn(dir)
    setTurn(t, 0)
    settle(t, true)
  }

  // ── the corners: no dog-ears (Rosson 2026-10-08) ─────────────────────
  // At rest a corner is plain paper. Reach for it (hover, or keyboard focus
  // on the turn control) and it lifts in a soft curl: a curved crease, the
  // underside lighter at the crease and darker toward the tip, grain on
  // the back, a soft shadow on the page beneath and the page under showing
  // through. Drawn once in SVG; CSS scales it from the corner (200 ms), so
  // every size of lift is the same fold. Touch: a faint static hint.
  // Reduced motion: a small still curl.
  var CURL = 64 // the drawing, CURL × CURL, the page's corner at its bottom right
  var CURL_LIFT = 40 // how far the fold reaches along each edge, fully lifted

  function curlSvg(name) {
    var S = CURL
    var A = CURL_LIFT
    var at = function (k) { return S - A * k }
    var el = function (tag, attrs, parent) {
      var n = document.createElementNS(SVG_NS, tag)
      Object.keys(attrs).forEach(function (k) { n.setAttribute(k, String(attrs[k])) })
      if (parent) parent.appendChild(n)
      return n
    }
    var svg = el('svg', { class: 'curl', viewBox: '0 0 ' + S + ' ' + S, 'aria-hidden': 'true', focusable: 'false' })
    var defs = el('defs', {}, svg)
    var under = el('linearGradient', { id: 'curl-under-' + name, gradientUnits: 'userSpaceOnUse', x1: at(0.42), y1: at(0.42), x2: at(1), y2: at(1) }, defs)
    el('stop', { offset: '0', 'stop-color': '#f7ecd2' }, under)
    el('stop', { offset: '0.55', 'stop-color': '#e2cc9e' }, under)
    el('stop', { offset: '1', 'stop-color': '#bfa271' }, under)
    var beneath = el('linearGradient', { id: 'curl-beneath-' + name, gradientUnits: 'userSpaceOnUse', x1: at(0.5), y1: at(0.5), x2: S, y2: S }, defs)
    el('stop', { offset: '0', 'stop-color': '#9c8358' }, beneath)
    el('stop', { offset: '1', 'stop-color': '#c7b085' }, beneath)
    var soft = el('filter', { id: 'curl-soft-' + name, x: '-50%', y: '-50%', width: '200%', height: '200%' }, defs)
    el('feGaussianBlur', { stdDeviation: '2.6' }, soft)
    var grain = el('filter', { id: 'curl-grain-' + name, x: '0', y: '0', width: '100%', height: '100%' }, defs)
    el('feTurbulence', { type: 'fractalNoise', baseFrequency: '0.9', numOctaves: '2', seed: name === 'next' ? '7' : '3', result: 'n' }, grain)
    el('feColorMatrix', { in: 'n', type: 'matrix', values: '0 0 0 0 0.35  0 0 0 0 0.24  0 0 0 0 0.12  0 0 0 0.22 0', result: 'g' }, grain)
    el('feComposite', { in: 'g', in2: 'SourceGraphic', operator: 'in', result: 'gi' }, grain)
    var merge = el('feMerge', {}, grain)
    el('feMergeNode', { in: 'SourceGraphic' }, merge)
    el('feMergeNode', { in: 'gi' }, merge)
    var p = function (x, y) { return x.toFixed(1) + ' ' + y.toFixed(1) }
    // The crease: from the bottom edge to the right edge, bowed a little
    // toward the corner (paper rolls; it doesn't crease flat).
    var crease = 'M' + p(at(1), S) + ' Q' + p(at(0.36), at(0.36)) + ' ' + p(S, at(1))
    // The flap: the corner turned back over the page, its tip rounded.
    var flap = crease + ' Q' + p(at(0.55), at(0.9)) + ' ' + p(at(0.97), at(0.97)) + ' Q' + p(at(0.9), at(0.55)) + ' ' + p(at(1), S) + ' Z'
    var lift = el('g', { class: 'lift' }, svg)
    el('path', { class: 'beneath', d: crease + ' L' + p(S, S) + ' Z', fill: 'url(#curl-beneath-' + name + ')' }, lift)
    el('path', { class: 'cast', d: flap, transform: 'translate(-2.5 -2.5)', filter: 'url(#curl-soft-' + name + ')' }, lift)
    el('path', { class: 'under', d: flap, fill: 'url(#curl-under-' + name + ')', filter: 'url(#curl-grain-' + name + ')' }, lift)
    el('path', { class: 'crease', d: crease }, lift)
    return svg
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

  // ── the pen scratches while the agent works ──────────────────────────
  // Rosson 2026-10-08: no loading dots. While the agent works, an unseen
  // hand scrawls half a word, strikes it, starts another, scratches it out
  // and tangles ink over the lot, then the page drinks it and it begins
  // again, never quite the same. SVG built with createElementNS, each
  // stroke drawn by its dash offset. It clears the instant the reply starts
  // writing; reduced motion shows one still mark; a hidden frame pauses it.

  var SCRIBE_W = 260 // the viewBox: 0 0 SCRIBE_W SCRIBE_H
  var SCRIBE_H = 44
  // Every point stays this far inside the viewBox: half the widest stroke,
  // the jitter already in the point, and the ink's 1px shadow, with room
  // to spare. The ancestors clip (the sheet scrolls), so nothing may poke
  // out (Rosson 2026-10-08: "cut off on the bottom and on the left").
  var SCRIBE_PAD = 4
  var SCRIBE_BASE = 27
  var SCRIBE_PX_PER_MS = 0.32

  var scribe = { on: false, paused: false, timer: 0, strokes: [], at: 0 }

  function pathOf(points) {
    return 'M' + points.map(function (p) { return p[0].toFixed(1) + ' ' + p[1].toFixed(1) }).join(' L')
  }

  function lengthOf(points) {
    var len = 0
    for (var i = 1; i < points.length; i++) len += Math.hypot(points[i][0] - points[i - 1][0], points[i][1] - points[i - 1][1])
    return len
  }

  /** Half a word in a cramped, looping hand. */
  function scrawl(x0, letters) {
    var pts = []
    var a = jitter(1.7, 0.9)
    var b = jitter(3, 1.6)
    var tall = []
    for (var l = 0; l <= letters; l++) tall.push(jitter(3, 7))
    for (var t = 0; t <= letters * Math.PI * 2; t += 0.35) {
      var h = tall[Math.floor(t / (Math.PI * 2))]
      pts.push([x0 + a * t - b * Math.sin(t) + jitter(-0.4, 0.8), SCRIBE_BASE - h * (1 - Math.cos(t)) / 2 + jitter(-0.5, 1)])
    }
    return pts
  }

  function strike(x0, x1) {
    var y0 = SCRIBE_BASE - jitter(2, 5)
    var y1 = y0 + jitter(-4, 8)
    var pts = []
    for (var i = 0; i <= 12; i++) {
      var k = i / 12
      pts.push([x0 - 4 + (x1 - x0 + 8) * k, y0 + (y1 - y0) * k + jitter(-0.8, 1.6)])
    }
    return pts
  }

  /** Back and forth over a word, hard and fast. */
  function scratch(x0, x1) {
    var pts = []
    var n = Math.round(jitter(9, 8))
    for (var i = 0; i <= n; i++) {
      var x = x0 + ((x1 - x0) * i) / n + jitter(-3, 6)
      pts.push([x, i % 2 ? SCRIBE_BASE + jitter(1, 4) : SCRIBE_BASE - jitter(10, 6)])
    }
    return pts
  }

  /** A furious knot of loops over everything. */
  function tangle(cx, w) {
    var pts = []
    var turns = jitter(3, 3)
    for (var t = 0; t <= turns * Math.PI * 2; t += 0.3) {
      var r = jitter(0.75, 0.5)
      pts.push([cx + Math.cos(t) * (w / 2) * r + (t / (turns * Math.PI * 2) - 0.5) * w * 0.6, SCRIBE_BASE - 6 + Math.sin(t * jitter(0.9, 0.3)) * 9 * r])
    }
    return pts
  }

  /** One fit of frustration: what to draw, in order. */
  function frustration() {
    var out = []
    var x = jitter(4, 10)
    var words = Math.random() < 0.5 ? 2 : 3
    var spans = []
    for (var i = 0; i < words && x < SCRIBE_W - 40; i++) {
      var pts = scrawl(x, Math.round(jitter(2, 3)))
      var end = pts[pts.length - 1][0]
      out.push({ pts: pts, cls: 'scrawl' })
      out.push({ pts: Math.random() < 0.55 ? strike(x, end) : scratch(x, end), cls: 'frantic' })
      spans.push([x, end])
      x = end + jitter(12, 14)
    }
    var last = spans[spans.length - 1]
    out.push({ pts: tangle((spans[0][0] + last[1]) / 2, last[1] - spans[0][0]), cls: 'frantic' })
    return out
  }

  /** Keep the ink inside the drawing, whatever the dice said. */
  function inside(points) {
    return points.map(function (q) {
      return [clamp(q[0], SCRIBE_PAD, SCRIBE_W - SCRIBE_PAD), clamp(q[1], SCRIBE_PAD, SCRIBE_H - SCRIBE_PAD)]
    })
  }

  function stroke(item, still) {
    item.pts = inside(item.pts)
    var p = document.createElementNS(SVG_NS, 'path')
    p.setAttribute('d', pathOf(item.pts))
    p.setAttribute('class', item.cls)
    if (still) return p
    var len = Math.ceil(lengthOf(item.pts))
    var ms = Math.round(clamp(len / (item.cls === 'frantic' ? SCRIBE_PX_PER_MS * 2.2 : SCRIBE_PX_PER_MS), 140, 1300))
    p.style.strokeDasharray = String(len)
    p.style.strokeDashoffset = String(len)
    p.style.animationDuration = ms + 'ms'
    item.ms = ms
    return p
  }

  function scribeClear() {
    clearTimeout(scribe.timer)
    scribe.timer = 0
    dom.scribble.textContent = ''
    dom.scribble.classList.remove('smear', 'still', 'paused')
  }

  function scribeStart() {
    if (scribe.on) return
    scribe.on = true
    scribe.paused = false
    scribeClear()
    if (reduced()) {
      // One still mark: a word struck through and scratched over.
      dom.scribble.classList.add('still')
      ;[
        { pts: [[8, 27], [14, 18], [18, 27], [24, 16], [29, 27], [36, 19], [41, 27], [48, 18], [54, 27]], cls: 'scrawl' },
        { pts: [[4, 23], [58, 20]], cls: 'frantic' },
        { pts: [[10, 15], [20, 30], [28, 14], [38, 31], [46, 15], [56, 29]], cls: 'frantic' },
      ].forEach(function (item) { dom.scribble.appendChild(stroke(item, true)) })
      return
    }
    scribe.strokes = frustration()
    scribe.at = 0
    scribeNext()
  }

  function scribeNext() {
    scribe.timer = 0
    if (!scribe.on || scribe.paused) return
    if (scribe.at < scribe.strokes.length) {
      var item = scribe.strokes[scribe.at++]
      dom.scribble.appendChild(stroke(item, false))
      scribe.timer = setTimeout(scribeNext, item.ms + jitter(40, 220))
      return
    }
    if (!dom.scribble.classList.contains('smear')) {
      // The page drinks the mess, and the hand starts over.
      dom.scribble.classList.add('smear')
      scribe.timer = setTimeout(scribeNext, jitter(500, 300))
      return
    }
    dom.scribble.textContent = ''
    dom.scribble.classList.remove('smear')
    scribe.strokes = frustration()
    scribe.at = 0
    scribe.timer = setTimeout(scribeNext, jitter(150, 250))
  }

  function scribeStop() {
    if (!scribe.on) return
    scribe.on = false
    scribeClear()
  }

  function scribePause(hidden) {
    if (!scribe.on || reduced()) return
    scribe.paused = hidden
    dom.scribble.classList.toggle('paused', hidden)
    clearTimeout(scribe.timer)
    scribe.timer = 0
    if (!hidden) scribe.timer = setTimeout(scribeNext, 200)
  }

  // ── how many ghosts are out, and how many scary things (Rosson 2026-10-08)
  // K2 0.45.2 gives each agent row `counts: {subagents, tools, commands}`:
  // ghosts are the live subagents, scary things this turn's tool calls.
  // An older K2 sends none, and a row that can't say sends null: then the
  // page says nothing extra, and never makes a number up.

  var NUMBER_WORDS = ['no', 'one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight', 'nine', 'ten', 'eleven', 'twelve']
  var ONE_GHOST = ['a ghost wanders off to look…', 'one ghost slips away to search…', 'a lone ghost drifts down the hall…']
  var SOME_GHOSTS = ['{n} ghosts are stirring…', '{n} ghosts whisper among themselves…', '{n} ghosts drift through the rooms…']
  var MANY_GHOSTS = ['the house is restless: {n} ghosts…', '{n} ghosts crowd the halls…', 'the walls are thick with them: {n} ghosts…']
  var MANY_AT = 6
  // After a ghost line, or alone. `{t}` is the exact count, in digits.
  var ONE_SCARY = ['1 scary thing so far', '1 scary thing has happened', 'the house has seen 1 scary thing']
  var SOME_SCARY = ['{t} scary things have happened', '{t} scary things so far', 'the house has seen {t} scary things']

  /** A count K2 sent: a whole number, zero or more; anything else is not one. */
  function whole(v) {
    return typeof v === 'number' && isFinite(v) && v >= 0 ? Math.floor(v) : null
  }

  /** `{n, t}` (ghosts, scary things) when K2 sent both and either is out;
   *  null when there are no counts, a malformed one, or nothing yet. */
  function counted(row) {
    var c = row && row.counts
    if (!c || typeof c !== 'object') return null
    var n = whole(c.subagents)
    var t = whole(c.tools)
    if (n === null || t === null || (n === 0 && t === 0)) return null
    return { n: n, t: t }
  }

  /** The line: ghosts first, then the scary things. Each part's wording
   *  varies by agent and holds still while its number does (the scary
   *  part by one vs many, so it doesn't jump on every call). */
  function ghostsLine(addr, c) {
    var pick = function (list, key) { return list[hash(addr + ':' + key) % list.length] }
    var ghosts = ''
    if (c.n === 1) ghosts = pick(ONE_GHOST, 'g1')
    else if (c.n > 1) {
      var words = c.n < NUMBER_WORDS.length ? NUMBER_WORDS[c.n] : String(c.n)
      ghosts = pick(c.n >= MANY_AT ? MANY_GHOSTS : SOME_GHOSTS, 'g' + c.n).replace('{n}', words)
    }
    if (c.t === 0) return ghosts
    var scary = c.t === 1 ? pick(ONE_SCARY, 's1') : pick(SOME_SCARY, 'sn').replace('{t}', String(c.t))
    return ghosts ? ghosts + ' ' + scary : scary + '…'
  }

  /** The scribble shows while the agent works, and never over a reply
   *  being written. */
  function stirSync() {
    var row = current()
    var view = row ? state.views[row.address] : null
    var on = !!row && !dom.page.classList.contains('blank') && isWorking(row, view) && !state.revealing
    var c = on ? counted(row) : null
    dom.ghosts.hidden = !c
    text(dom.ghosts, c ? ghostsLine(row.address, c) : '')
    dom.stir.hidden = !on
    if (on) scribeStart()
    else scribeStop()
    glideOn()
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
  var GHOST_IDLE_MS = 3500

  // phase: 'gap' (between lines), 'write', 'erase' (lingers first).
  var ghost = { woke: false, on: false, frozen: false, timer: 0, idle: 0, line: -1, words: '', n: 0, phase: 'gap' }

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
    ghost.frozen = false
    ghost.words = ''
    ghost.n = 0
    ghost.phase = 'gap'
    dom.ghost.textContent = ''
    dom.ghost.hidden = true
    dom.ghost.classList.remove('still', 'frozen')
  }

  /** Paint the first `n` letters of the line, each a little unsteady. */
  function ghostPaint(words, n) {
    dom.ghost.textContent = ''
    for (var i = 0; i < n; i++) {
      var ch = make('span', i === n - 1 && !ghost.frozen ? 'gl fresh' : 'gl', words.charAt(i))
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
    ghost.woke = true
    dom.ghost.hidden = false
    if (reduced()) {
      dom.ghost.classList.add('still')
      dom.ghost.textContent = GHOST_STILL
      return
    }
    ghostAfter(0)
  }

  function ghostAfter(ms) {
    clearTimeout(ghost.timer)
    ghost.timer = setTimeout(ghostStep, ms)
  }

  function ghostStep() {
    ghost.timer = 0
    if (!ghost.on || ghost.frozen) return
    if (ghost.phase === 'gap') {
      var next = Math.floor(Math.random() * GHOST_LINES.length)
      if (next === ghost.line) next = (next + 1) % GHOST_LINES.length
      ghost.line = next
      ghost.words = GHOST_LINES[next]
      ghost.n = 0
      ghost.phase = 'write'
      return ghostAfter(jitter(120, 240))
    }
    if (ghost.phase === 'write') {
      ghost.n++
      ghostPaint(ghost.words, ghost.n)
      if (ghost.n >= ghost.words.length) {
        ghost.phase = 'erase'
        return ghostAfter(jitter(1600, 1800)) // it lingers
      }
      // The hand hesitates at a pause in the thought.
      var c = ghost.words.charAt(ghost.n - 1)
      return ghostAfter(jitter(55, 110) + (/[….,?]/.test(c) ? jitter(140, 260) : 0))
    }
    ghost.n--
    ghostPaint(ghost.words, ghost.n)
    if (ghost.n <= 0) {
      ghost.phase = 'gap'
      return ghostAfter(jitter(700, 1300))
    }
    ghostAfter(jitter(28, 42))
  }

  /** You touched the pen: the ghost holds still, its line whole and faint,
   *  until your first letter (Rosson 2026-10-08: selecting the pen never
   *  makes writing disappear). */
  function ghostFreeze() {
    clearTimeout(ghost.idle)
    ghost.idle = 0
    if (!ghost.on || reduced()) return
    clearTimeout(ghost.timer)
    ghost.timer = 0
    ghost.frozen = true
    if (!ghost.words) {
      ghost.words = GHOST_STILL
      ghost.line = 0
    }
    ghost.n = ghost.words.length
    ghost.phase = 'erase'
    dom.ghost.classList.add('frozen')
    ghostPaint(ghost.words, ghost.n)
  }

  /** Bring the ghost back (or set a frozen one moving) after the pen has
   *  been empty and left alone a while. */
  function ghostLater(ms, restart) {
    if (ghost.on && !ghost.frozen) return
    if (ghost.idle && !restart) return
    clearTimeout(ghost.idle)
    ghost.idle = setTimeout(function () {
      ghost.idle = 0
      if (ghost.frozen) {
        if (!ghostMay(false)) return
        ghost.frozen = false
        dom.ghost.classList.remove('frozen')
        ghostAfter(jitter(300, 400))
      } else {
        ghostStart(false)
      }
    }, ms)
  }

  /** After a redraw: the pen may be gone, or hold a draft. */
  function ghostSync() {
    if (!ghostMay(true)) return ghostStop()
    if (document.activeElement === dom.ink) return
    // No "write here" ever (Rosson 2026-10-08): an empty pen shows the
    // ghost from the first moment; later it returns after the idle wait.
    if (!ghost.woke) ghostStart(false)
    else ghostLater(GHOST_IDLE_MS, false)
  }

  // ── writing: the ink sinks into the page ─────────────────────────────

  function send() {
    var words = dom.ink.value.trim()
    var addr = address()
    if (!words || state.sending || !addr || state.turning) return
    if (!reduced()) fxSpot(dom.ink, FX_SOAK_MS)
    state.sending = true
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
      })
  }

  // ── seen or not ──────────────────────────────────────────────────────
  // Everything that moves or sounds stops when the Diary can't be seen: the
  // window hidden (visibilitychange), or this frame laid out of sight
  // (IntersectionObserver, a second lock; a Garden switch already removes
  // the frame). Without an observer, a shown document counts as seen.

  var sight = { io: true, observer: null }

  function shown() {
    return !document.hidden && sight.io
  }

  function watchSight() {
    if (typeof window.IntersectionObserver !== 'function') return
    sight.observer = new window.IntersectionObserver(function (seen) {
      var last = seen[seen.length - 1]
      var io = !!last && (last.isIntersecting || last.intersectionRatio > 0)
      if (io === sight.io) return
      sight.io = io
      sightChanged()
    })
    sight.observer.observe(dom.desk)
  }

  function sightChanged() {
    var hidden = !shown()
    dom.root.classList.toggle('asleep', hidden)
    scribePause(hidden)
    if (hidden) ghostStop()
    else ghostLater(GHOST_IDLE_MS, true)
    flameSync()
    fxSync()
    musicSync()
  }

  // ── the paper under the words (PixiJS) ───────────────────────────────
  // One WebGL canvas, first in the page, under every word (the text stays
  // HTML: sharp, selectable, read aloud). One fragment shader paints:
  //   - paper fibre and grain (a 256 px tile made once, repeated);
  //   - the candle warming the page, from the one flame (below) that also
  //     drives the room's candle and the page's CSS glow;
  //   - ink bleeding into the fibres under freshly written words: while a
  //     reply bleeds in, the wet words' box is sampled every FX_SAMPLE_MS
  //     (a read before the frame's writes, never in the paint loop) and
  //     spreads and dries over FX_WET_MS; sent words soak a stain too.
  // Frames are drawn by the flame's clock (FLAME_FPS, under the 30 fps
  // cap), never by a free-running ticker. It all stops when the Diary
  // can't be seen; reduced motion draws one still frame. The canvas covers
  // the page box edge to edge: sized from the page's own rect on every
  // resize (a ResizeObserver), at the screen's density. No WebGL, no Pixi:
  // the canvas goes and the CSS page stays.

  var FX_SPOTS = 6
  var FX_SAMPLE_MS = 140
  var FX_WET_MS = 1800
  var FX_SOAK_MS = 3200
  var FX_TILE = 256
  // ── one flame (Rosson 2026-10-08: "sync them up") ────────────────────
  // Every candle effect follows ONE flame: the candle in the room behind
  // the book, the page's CSS glow and the paper shader's light. The flame
  // is a seeded noise of time, sampled once a frame at FLAME_FPS; that one
  // sample sets the room candle's opacity and size, and the page light (the
  // same flame, a frame softer and dimmer: it follows, never leads) sets
  // the glow's opacity and the shader's light in the same frame. CSS only
  // eases within a frame (FLAME_EASE_MS). Hidden or reduced motion: all of
  // them hold FLAME_REST together.

  var FLAME_FPS = 20
  var FLAME_EASE_MS = 50
  var FLAME_REST = 0.9
  var FLAME_FOLLOW = 0.6 // how much of the flame's change the page light takes each frame
  var FX_REST_FPS = 10

  var flame = { on: false, frame: 0, last: 0, value: FLAME_REST, page: FLAME_REST }

  /** A smooth seeded noise in [0, 1). */
  function lattice(i) {
    var h = Math.imul((i | 0) ^ 0x2f6b9e53, 0x45d9f3b)
    h = Math.imul(h ^ (h >>> 16), 0x45d9f3b)
    return ((h ^ (h >>> 16)) >>> 0) / 4294967296
  }

  function noise1(x) {
    var i = Math.floor(x)
    var f = x - i
    var u = f * f * (3 - 2 * f)
    return lattice(i) * (1 - u) + lattice(i + 1) * u
  }

  /** The flame's brightness at `ms`: a restless flicker, now and then a
   *  gutter that nearly puts it out. In [0.35, 1]. */
  function flameAt(ms) {
    var t = ms / 1000
    var v = 0.84 + 0.08 * (noise1(t * 3.3) - 0.5) * 2 + 0.05 * (noise1(t * 9.7 + 17) - 0.5) * 2
    var g = noise1(t * 0.42 + 91)
    if (g > 0.72) v -= ((g - 0.72) / 0.28) * 0.4
    return clamp(v, 0.35, 1)
  }

  /** One sample, everywhere at once. */
  function flameApply(v, now, still) {
    flame.value = v
    flame.page = still ? v : flame.page + (v - flame.page) * FLAME_FOLLOW
    if (dom.candle) {
      dom.candle.style.opacity = v.toFixed(3)
      dom.candle.style.transform = 'scale(' + (0.93 + 0.1 * v).toFixed(3) + ')'
    }
    dom.glow.style.opacity = (0.45 + 0.55 * flame.page).toFixed(3)
    // The paper redraws with the same sample: every flame frame while ink
    // is wet; at rest only when its light has visibly moved, at most
    // FX_REST_FPS a second (it never runs ahead of the flame).
    if (still || fx.spots.length || (now - fx.drawnAt >= 1000 / FX_REST_FPS && Math.abs(flame.page - fx.drawn) >= 0.006)) fxDraw(now, still)
  }

  function flameTick(now) {
    flame.frame = 0
    if (!flame.on) return
    if (now - flame.last >= 1000 / FLAME_FPS - 2) {
      flame.last = now
      flameApply(flameAt(now), now, false)
    }
    flame.frame = requestAnimationFrame(flameTick)
  }

  /** Burn while seen and moving; otherwise hold every candle at rest. */
  function flameSync() {
    var on = shown() && !reduced()
    if (on && !flame.on) {
      flame.on = true
      flame.last = -Infinity
      flame.frame = requestAnimationFrame(flameTick)
    } else if (!on) {
      flame.on = false
      if (flame.frame) cancelAnimationFrame(flame.frame)
      flame.frame = 0
      flameApply(FLAME_REST, performance.now(), true)
    }
  }

  var FX_VERTEX = [
    'in vec2 aPosition;',
    'out vec2 vUV;',
    'void main(void) {',
    '  vUV = vec2(aPosition.x * 0.5 + 0.5, 0.5 - aPosition.y * 0.5);',
    '  gl_Position = vec4(aPosition, 0.0, 1.0);',
    '}',
  ].join('\n')

  var FX_FRAGMENT = [
    'in vec2 vUV;',
    'out vec4 finalColor;',
    'uniform sampler2D uPaper;',
    'uniform vec2 uSize;',
    'uniform vec4 uLight;',
    'uniform vec4 uSheet;',
    'uniform vec4 uSpot0;',
    'uniform vec4 uSpot1;',
    'uniform vec4 uSpot2;',
    'uniform vec4 uSpot3;',
    'uniform vec4 uSpot4;',
    'uniform vec4 uSpot5;',
    'uniform vec4 uWetA;',
    'uniform vec4 uWetB;',
    'float bleed(vec4 s, float wet, vec2 p, float fib) {',
    '  if (wet <= 0.0) return 0.0;',
    '  vec2 q = abs(p - (s.xy + s.zw * 0.5)) - s.zw * 0.5;',
    '  float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);',
    '  float reach = 1.0 + 5.0 * (1.0 - wet);',
    '  d -= fib * 4.0 * (1.0 - wet * 0.5);',
    '  return (1.0 - smoothstep(-2.0, reach, d)) * wet;',
    '}',
    'void main(void) {',
    '  vec2 px = vUV * uSize;',
    '  vec4 paper = texture(uPaper, px / ' + FX_TILE.toFixed(1) + ');',
    '  float grain = paper.r - 0.5;',
    '  float fib = paper.g;',
    '  vec2 lc = uLight.xy * uSize;',
    '  float lit = 1.0 - smoothstep(0.0, uLight.z * uSize.x, distance(px, lc));',
    '  lit = lit * lit * uLight.w;',
    '  float ga = abs(grain) * (0.16 + 0.22 * lit) + fib * 0.035;',
    '  vec3 col = (grain < 0.0 ? vec3(0.20, 0.11, 0.04) : vec3(1.0, 0.93, 0.78)) * ga;',
    '  float a = ga;',
    '  float wa = lit * 0.20;',
    '  col = vec3(1.0, 0.70, 0.36) * wa + col * (1.0 - wa);',
    '  a = wa + a * (1.0 - wa);',
    '  float b = bleed(uSpot0, uWetA.x, px, fib);',
    '  b = max(b, bleed(uSpot1, uWetA.y, px, fib));',
    '  b = max(b, bleed(uSpot2, uWetA.z, px, fib));',
    '  b = max(b, bleed(uSpot3, uWetA.w, px, fib));',
    '  b = max(b, bleed(uSpot4, uWetB.x, px, fib));',
    '  b = max(b, bleed(uSpot5, uWetB.y, px, fib));',
    '  float inside = step(uSheet.x, px.x) * step(px.x, uSheet.z) * step(uSheet.y, px.y) * step(px.y, uSheet.w);',
    '  float ba = clamp(b * inside * (0.05 + 0.13 * fib), 0.0, 0.2);',
    '  col = vec3(0.16, 0.06, 0.04) * ba + col * (1.0 - ba);',
    '  a = ba + a * (1.0 - ba);',
    '  float edge = min(min(px.x, uSize.x - px.x), min(px.y, uSize.y - px.y));',
    '  float worn = (1.0 - smoothstep(0.0, 7.0, edge + (fib - 0.5) * 7.0 + grain * 4.0)) * 0.24;',
    '  col = vec3(0.30, 0.18, 0.07) * worn + col * (1.0 - worn);',
    '  a = worn + a * (1.0 - worn);',
    '  finalColor = vec4(col, a);',
    '}',
  ].join('\n')

  var fx = {
    app: null,
    canvas: null,
    group: null,
    ready: false,
    failed: false,
    spots: [],
    drawn: -1, // the page light last drawn
    drawnAt: -Infinity, // {x, y, w, h, scroll, born, ms}: page px, the sheet's scroll then
    sampled: 0,
    w: 0,
    h: 0,
    sheet: [0, 0, 0, 0],
    scroll: 0,
    start: 0,
    observer: null,
  }

  /** The paper's tile, made once: red = grain (0.5 is none), green =
   *  fibre. Drawn wrapped, so it repeats without a seam. */
  function paperTile() {
    var c = document.createElement('canvas')
    c.width = FX_TILE
    c.height = FX_TILE
    var g = c.getContext('2d')
    if (!g) return null
    var img = g.createImageData(FX_TILE, FX_TILE)
    var roll = dice(0x5eed)
    for (var i = 0; i < img.data.length; i += 4) {
      img.data[i] = 128 + Math.round((roll() + roll() + roll() - 1.5) * 46)
      img.data[i + 1] = 0
      img.data[i + 2] = 0
      img.data[i + 3] = 255
    }
    g.putImageData(img, 0, 0)
    g.globalCompositeOperation = 'lighter'
    g.lineCap = 'round'
    for (var f = 0; f < 520; f++) {
      var x = roll() * FX_TILE
      var y = roll() * FX_TILE
      var len = 6 + roll() * 34
      var ang = (roll() - 0.5) * 1.1 + (roll() < 0.18 ? Math.PI / 2 : 0)
      var bend = (roll() - 0.5) * len * 0.5
      g.strokeStyle = 'rgba(0, ' + Math.round(60 + roll() * 150) + ', 0, 1)'
      g.lineWidth = 0.5 + roll() * 1.1
      for (var ox = -FX_TILE; ox <= FX_TILE; ox += FX_TILE) {
        for (var oy = -FX_TILE; oy <= FX_TILE; oy += FX_TILE) {
          var x0 = x + ox
          var y0 = y + oy
          g.beginPath()
          g.moveTo(x0, y0)
          g.quadraticCurveTo(x0 + Math.cos(ang) * len * 0.5 - Math.sin(ang) * bend, y0 + Math.sin(ang) * len * 0.5 + Math.cos(ang) * bend, x0 + Math.cos(ang) * len, y0 + Math.sin(ang) * len)
          g.stroke()
        }
      }
    }
    return c
  }

  /** The page's whole box (its own rect, so nothing short of an edge) and
   *  the sheet's box in it. A ResizeObserver callback or the first draw:
   *  layout is clean then. */
  function fxMeasure() {
    var r = dom.page.getBoundingClientRect()
    fx.w = Math.ceil(r.width)
    fx.h = Math.ceil(r.height)
    fx.sheet = [dom.sheet.offsetLeft, dom.sheet.offsetTop, dom.sheet.offsetLeft + dom.sheet.clientWidth, dom.sheet.offsetTop + dom.sheet.clientHeight]
  }

  function fxInit() {
    var P = window.PIXI
    if (fx.app || fx.failed || !P || typeof P.Application !== 'function') return
    var canvas = document.createElement('canvas')
    canvas.className = 'paperfx'
    canvas.setAttribute('aria-hidden', 'true')
    dom.page.insertBefore(canvas, dom.page.firstChild)
    fx.canvas = canvas
    fxMeasure()
    var app = new P.Application()
    fx.app = app
    var fail = function () {
      fx.failed = true
      fx.ready = false
      fx.app = null
      if (canvas.parentNode) canvas.parentNode.removeChild(canvas)
      dom.root.classList.remove('fx')
      try {
        app.destroy()
      } catch (_e) {
        // Half-made: nothing more to free.
      }
    }
    var made
    try {
      made = app.init({
        canvas: canvas,
        width: Math.max(1, fx.w),
        height: Math.max(1, fx.h),
        backgroundAlpha: 0,
        antialias: false,
        autoDensity: true,
        resolution: Math.min(window.devicePixelRatio || 1, 2),
        preference: 'webgl',
        powerPreference: 'low-power',
        autoStart: false,
        sharedTicker: false,
      })
    } catch (e) {
      fail()
      return
    }
    Promise.resolve(made)
      .then(function () {
        var tile = paperTile()
        if (!tile) throw new Error('no 2d canvas')
        var paper = P.Texture.from(tile)
        paper.source.addressMode = 'repeat'
        var group = new P.UniformGroup({
          uSize: { value: new Float32Array([fx.w, fx.h]), type: 'vec2<f32>' },
          uLight: { value: new Float32Array([0.16, 0.06, 0.95, 1]), type: 'vec4<f32>' },
          uSheet: { value: new Float32Array(fx.sheet), type: 'vec4<f32>' },
          uSpot0: { value: new Float32Array(4), type: 'vec4<f32>' },
          uSpot1: { value: new Float32Array(4), type: 'vec4<f32>' },
          uSpot2: { value: new Float32Array(4), type: 'vec4<f32>' },
          uSpot3: { value: new Float32Array(4), type: 'vec4<f32>' },
          uSpot4: { value: new Float32Array(4), type: 'vec4<f32>' },
          uSpot5: { value: new Float32Array(4), type: 'vec4<f32>' },
          uWetA: { value: new Float32Array(4), type: 'vec4<f32>' },
          uWetB: { value: new Float32Array(4), type: 'vec4<f32>' },
        })
        var shader = P.Shader.from({
          gl: { vertex: FX_VERTEX, fragment: FX_FRAGMENT, name: 'diary-paper' },
          resources: { fx: group, uPaper: paper.source },
        })
        var quad = new P.Geometry({ attributes: { aPosition: [-1, -1, 1, -1, 1, 1, -1, 1] }, indexBuffer: [0, 1, 2, 0, 2, 3] })
        var mesh = new P.Mesh({ geometry: quad, shader: shader })
        app.stage.addChild(mesh)
        fx.group = group
        fx.start = performance.now()
        fx.ready = true
        dom.root.classList.add('fx')
        if (typeof window.ResizeObserver === 'function') {
          fx.observer = new window.ResizeObserver(fxResized)
          fx.observer.observe(dom.page)
        } else {
          window.addEventListener('resize', fxResized)
        }
        fxSync()
      })
      .catch(fail)
  }

  function fxResized() {
    if (!fx.ready) return
    fxMeasure()
    fx.app.renderer.resize(Math.max(1, fx.w), Math.max(1, fx.h))
    if (shown()) fxDraw(performance.now(), !flame.on)
  }

  /** Draw a frame with the flame's page light. `still`: no wet ink (the
   *  candle at rest, reduced motion or hidden). */
  function fxDraw(now, still) {
    var u = fx.group && fx.group.uniforms
    if (!fx.ready || !u) return
    u.uSize[0] = fx.w
    u.uSize[1] = fx.h
    for (var s = 0; s < 4; s++) u.uSheet[s] = fx.sheet[s]
    var light = flame.page
    // The light leans a hair as the flame bends.
    u.uLight[0] = 0.16 + (light - FLAME_REST) * 0.03
    u.uLight[1] = 0.06 + (light - FLAME_REST) * 0.02
    u.uLight[2] = 0.95 * (0.96 + 0.04 * light)
    u.uLight[3] = light
    if (still) fx.spots = []
    fx.spots = fx.spots.filter(function (sp) { return now - sp.born < sp.ms })
    for (var i = 0; i < FX_SPOTS; i++) {
      var sp = fx.spots[fx.spots.length - 1 - i]
      var v = u['uSpot' + i]
      var wet = sp ? 1 - (now - sp.born) / sp.ms : 0
      v[0] = sp ? sp.x : 0
      v[1] = sp ? sp.y - (fx.scroll - sp.scroll) : 0
      v[2] = sp ? sp.w : 0
      v[3] = sp ? sp.h : 0
      ;(i < 4 ? u.uWetA : u.uWetB)[i % 4] = wet
    }
    fx.group.update()
    if (!shown()) return // set, ready for the return; nothing to draw unseen
    fx.app.render()
    fx.drawn = light
    fx.drawnAt = now
  }

  /** The paper follows the flame; just draw once now (the flame's clock
   *  draws the rest). */
  function fxSync() {
    if (fx.ready && shown()) fxDraw(performance.now(), !flame.on)
  }

  /** Wet ink under `el` (its box now), drying over `ms`. Called before the
   *  frame writes anything, so the read finds layout already done. */
  function fxSpot(el, ms) {
    if (!fx.ready || !flame.on || !el) return
    var r = el.getBoundingClientRect()
    if (!r.width || !r.height) return
    var p = dom.page.getBoundingClientRect()
    fx.spots.push({ x: r.left - p.left, y: r.top - p.top, w: r.width, h: r.height, scroll: fx.scroll, born: performance.now(), ms: ms })
    if (fx.spots.length > FX_SPOTS) fx.spots.shift()
  }

  /** While a reply bleeds in: the wet words now, at most every FX_SAMPLE_MS. */
  function fxWet(now, ink) {
    if (!flame.on || now - fx.sampled < FX_SAMPLE_MS) return
    fx.sampled = now
    for (var i = ink.nodes.length - 1; i >= 0; i--) {
      if (ink.nodes[i].wet.firstChild) return fxSpot(ink.nodes[i].wet, FX_WET_MS)
    }
  }

  function fxClear() {
    fx.spots = []
  }

  // ── the room's sound (Tone.js) ───────────────────────────────────────
  // Made in code, so there are no sound files: a slow low drone under a
  // sweeping filter, distant detuned music-box bells through an echo, a
  // floorboard's creak, and wind that swells and dies, all in one long
  // reverb. Quiet: the room sits at MUSIC_DB.
  //
  // It plays while you are on this Garden: it fades in when the Diary is
  // shown (K2 mounts the frame when you arrive and removes it when you
  // leave) and fades out when it is hidden. If the computer holds sound
  // back until a click, it starts on your first click or key in the Diary.
  // The speaker at the top left (or M, when you aren't writing) mutes it.
  // Only Tone's plain nodes are used: AudioWorklet nodes can't load in a
  // sealed widget.

  var MUSIC_DB = -24
  var MUSIC_IN_S = 4
  var MUSIC_OUT_S = 0.8
  // D minor with a flat second, high and far away.
  var BELL_NOTES = [74, 75, 77, 81, 82, 84, 86, 89]

  // The Diary's one remembered choice: the music's mute. K2 has no storage
  // verb for widgets yet, so it is kept here, in memory, for as long as the
  // frame lives. A Garden can start it muted: `music = "off"` in the
  // widget's config (k2.config).
  var prefs = (function () {
    var memory = {}
    return {
      get: function (key, fallback) {
        return Object.prototype.hasOwnProperty.call(memory, key) ? memory[key] : fallback
      },
      set: function (key, value) {
        memory[key] = value
      },
    }
  })()

  var music = { arrived: false, muted: false, playing: false, blocked: false, failed: false, nudged: false, ctx: null, n: null, timers: [], rest: 0 }

  function musicLoaded() {
    var T = window.Tone
    return !!T && typeof T.Context === 'function' && typeof T.setContext === 'function'
  }

  /** What the music is doing, in words (the speaker's tooltip and name). */
  function musicState() {
    if (!musicLoaded()) return 'the music library didn’t load'
    if (music.failed) return 'no Web Audio here'
    if (music.muted) return 'muted'
    if (music.playing) return 'playing'
    if (!music.arrived) return 'starting'
    if (!shown()) return 'resting while the Diary is hidden'
    if (music.blocked) return music.nudged ? 'blocked by the browser' : 'waiting for a click'
    return 'starting'
  }

  function musicAvailable() {
    var T = window.Tone
    return !music.failed && !!T && typeof T.Context === 'function' && typeof T.setContext === 'function'
  }

  function musicWanted() {
    return music.arrived && shown() && !music.muted && musicAvailable()
  }

  function dbGain(db) {
    return Math.pow(10, db / 20)
  }

  function midiHz(m, cents) {
    return 440 * Math.pow(2, (m - 69) / 12 + (cents || 0) / 1200)
  }

  /** The audio context, made on the first arrival (nothing sounds yet). */
  function musicContext() {
    var T = window.Tone
    var ctx = new T.Context({ latencyHint: 'playback', lookAhead: 0.25, updateInterval: 0.1 })
    T.setContext(ctx)
    music.ctx = ctx
  }

  /** Build the room once, the first time the context runs. */
  function musicBuild() {
    var T = window.Tone
    var master = new T.Gain(0).toDestination()
    var room = new T.Reverb({ decay: 6, preDelay: 0.08, wet: 0.6 }).connect(master)
    var echo = new T.FeedbackDelay({ delayTime: 0.43, feedback: 0.32, wet: 0.35 }).connect(room)
    var dry = new T.Gain(0.35).connect(master)
    // The drone: three low voices a few cents apart, under a slow filter,
    // in breaths (droneSwell), not one endless tone.
    var drone = new T.Gain(0)
    drone.connect(room)
    drone.connect(dry)
    var low = new T.Filter({ type: 'lowpass', frequency: 420, Q: 0.7 }).connect(drone)
    var voices = [
      new T.Oscillator({ frequency: midiHz(38), type: 'sine', volume: -4 }),
      new T.Oscillator({ frequency: midiHz(45, 7), type: 'triangle', volume: -8 }),
      new T.Oscillator({ frequency: midiHz(50, -9), type: 'sawtooth', volume: -16 }),
    ]
    voices.forEach(function (v) { v.connect(low) })
    var sweep = new T.LFO({ frequency: 1 / 23, min: 260, max: 700 }).connect(low.frequency)
    // Wind: pink noise in a moving band.
    var windBand = new T.Filter({ type: 'bandpass', frequency: 460, Q: 1.1 })
    var wind = new T.Gain(0.03).connect(room)
    windBand.connect(wind)
    var air = new T.Noise({ type: 'pink', volume: 0 }).connect(windBand)
    // Bells: a small FM bell, a little out of tune.
    var bells = new T.PolySynth(T.FMSynth, {
      harmonicity: 3.01,
      modulationIndex: 7,
      oscillator: { type: 'sine' },
      modulation: { type: 'sine' },
      envelope: { attack: 0.002, decay: 1.6, sustain: 0, release: 1.8 },
      modulationEnvelope: { attack: 0.002, decay: 0.5, sustain: 0, release: 0.5 },
      volume: -2,
    }).connect(echo)
    // A creak: a slow sawtooth (the stick and slip) through a wooden band.
    var creakPan = new T.Panner(0).connect(room)
    creakPan.connect(dry)
    var creakEnv = new T.AmplitudeEnvelope({ attack: 0.06, decay: 0.2, sustain: 0.8, release: 0.3 }).connect(creakPan)
    var creakBand = new T.Filter({ type: 'bandpass', frequency: 650, Q: 7 }).connect(creakEnv)
    var creaker = new T.Oscillator({ frequency: 30, type: 'sawtooth', volume: -10 }).connect(creakBand)
    voices.forEach(function (v) { v.start() })
    sweep.start()
    air.start()
    creaker.start()
    music.n = { drone: drone, voices: voices, master: master, wind: wind, windBand: windBand, bells: bells, creakPan: creakPan, creakEnv: creakEnv, creakBand: creakBand, creaker: creaker }
  }

  /** Something in the room, again after `lo`..`lo + spread` seconds. */
  function musicLater(fn, lo, spread) {
    var id = setTimeout(function () {
      music.timers = music.timers.filter(function (t) { return t !== id })
      if (music.playing) fn()
    }, Math.round(jitter(lo, spread) * 1000))
    music.timers.push(id)
  }

  // Rosson 2026-10-08: "the low tone goes for a bit too long". It breathes:
  // swells in over 3-5 s, holds 8-15 s, fades over 4-7 s, then rests 10-25
  // s (wind and creaks only). Each breath sits a little higher or lower.
  var DRONE_ROOTS = [38, 38, 36, 41, 33, 40]

  function droneSwell() {
    var T = window.Tone
    var n = music.n
    var at = T.now() + 0.05
    var root = DRONE_ROOTS[Math.floor(Math.random() * DRONE_ROOTS.length)]
    var cents = jitter(-15, 30)
    n.voices[0].frequency.setValueAtTime(midiHz(root, cents), at)
    n.voices[1].frequency.setValueAtTime(midiHz(root + 7, cents + 7), at)
    n.voices[2].frequency.setValueAtTime(midiHz(root + 12, cents - 9), at)
    var rise = jitter(3, 2)
    var hold = jitter(8, 7)
    var fall = jitter(4, 3)
    n.drone.gain.rampTo(jitter(0.75, 0.25), rise)
    musicLater(function () {
      n.drone.gain.rampTo(0, fall)
    }, rise + hold, 0)
    musicLater(droneSwell, rise + hold + fall + 10, 15)
  }

  function bellPhrase() {
    var T = window.Tone
    var at = T.now() + 0.05
    var notes = 2 + Math.floor(Math.random() * 3)
    var winding = Math.random() < 0.3 // a music box running down
    for (var i = 0; i < notes; i++) {
      var m = BELL_NOTES[Math.floor(Math.random() * BELL_NOTES.length)]
      var cents = jitter(-25, 50) - (winding ? i * 14 : 0)
      music.n.bells.triggerAttackRelease(midiHz(m, cents), 1.2, at, jitter(0.25, 0.4))
      at += jitter(0.38, 0.6) * (winding ? 1 + i * 0.25 : 1)
    }
    musicLater(bellPhrase, 6, 9)
  }

  function creak() {
    var T = window.Tone
    var n = music.n
    var at = T.now() + 0.05
    var dur = jitter(0.5, 0.9)
    n.creaker.frequency.setValueAtTime(jitter(18, 10), at)
    n.creaker.frequency.linearRampToValueAtTime(jitter(36, 20), at + dur)
    n.creakBand.frequency.setValueAtTime(jitter(480, 400), at)
    n.creakPan.pan.setValueAtTime(jitter(-0.8, 1.6), at)
    n.creakEnv.triggerAttackRelease(dur, at)
    musicLater(creak, 16, 26)
  }

  function windSwell() {
    var n = music.n
    n.wind.gain.rampTo(jitter(0.1, 0.08), 5)
    n.windBand.frequency.rampTo(jitter(750, 300), 5)
    musicLater(function () {
      n.wind.gain.rampTo(0.03, 9)
      n.windBand.frequency.rampTo(460, 9)
    }, 6, 2)
    musicLater(windSwell, 20, 25)
  }

  /** Fade in (or back in); if sound is held back, wait for a gesture. */
  function musicPlay() {
    clearTimeout(music.rest)
    music.rest = 0
    var silent = function () {
      // No Web Audio here: the Diary stays silent; the speaker says so.
      music.failed = true
      music.playing = false
      hushDraw()
    }
    if (!music.ctx) {
      try {
        musicContext()
      } catch (_e) {
        return silent()
      }
    }
    var ctx = music.ctx
    var go = function () {
      if (music.ctx !== ctx) return
      if (!musicWanted()) {
        if (!music.playing && ctx.state === 'running' && ctx.rawContext && typeof ctx.rawContext.suspend === 'function') ctx.rawContext.suspend()
        return
      }
      if (ctx.state !== 'running') {
        music.blocked = true
        hushDraw()
        return
      }
      music.blocked = false
      if (!music.n) {
        try {
          musicBuild()
        } catch (_e) {
          return silent()
        }
      }
      ctx.updateInterval = 0.1
      music.n.master.gain.rampTo(dbGain(MUSIC_DB), MUSIC_IN_S)
      if (music.playing) return
      music.playing = true
      hushDraw()
      music.n.drone.gain.rampTo(0, 1.5) // a breath cut short by leaving ends
      musicLater(droneSwell, 1, 2)
      musicLater(bellPhrase, 2, 4)
      musicLater(creak, 8, 12)
      musicLater(windSwell, 5, 10)
    }
    // Until it runs, a click or key in the Diary may start it (a resume
    // held back for a gesture may never settle, so don't wait to know).
    if (ctx.state !== 'running') music.blocked = true
    var resumed
    try {
      resumed = ctx.resume()
    } catch (_e) {
      resumed = null
    }
    Promise.resolve(resumed).then(go, go)
  }

  /** Fade out, then let the audio rest (a suspended context costs nothing). */
  function musicRest() {
    music.blocked = false
    if (!music.ctx) return
    var was = music.playing
    music.playing = false
    music.timers.forEach(clearTimeout)
    music.timers = []
    if (!was || music.rest || !music.n) return
    var ctx = music.ctx
    music.n.master.gain.rampTo(0, MUSIC_OUT_S)
    music.rest = setTimeout(function () {
      music.rest = 0
      if (music.playing || music.ctx !== ctx) return
      ctx.updateInterval = 1
      var raw = ctx.rawContext
      if (raw && typeof raw.suspend === 'function') raw.suspend()
    }, Math.round(MUSIC_OUT_S * 1000) + 100)
  }

  function musicSync() {
    if (musicAvailable()) {
      if (musicWanted()) musicPlay()
      else musicRest()
    }
    hushDraw()
  }

  /** The frame is going away: stop at once. */
  function musicClose() {
    musicRest()
    clearTimeout(music.rest)
    music.rest = 0
    var ctx = music.ctx
    music.ctx = null
    if (ctx && typeof ctx.close === 'function') {
      try {
        ctx.close()
      } catch (_e) {
        // Already closed.
      }
    }
  }

  function musicMute(muted) {
    music.muted = !!muted
    prefs.set('music.muted', music.muted)
    hushDraw()
    musicSync()
  }

  /** The speaker: a hand-inked cone and its sound; muted, it is crossed
   *  out. Built once, its waves and cross shown by class. */
  function hushDraw() {
    var b = dom.hush
    var can = musicAvailable()
    var words = (music.muted || !can ? 'Play the music (M)' : 'Mute the music (M)') + '. Music: ' + musicState()
    b.setAttribute('aria-pressed', music.muted ? 'true' : 'false')
    b.setAttribute('aria-label', words)
    b.setAttribute('title', words)
    b.classList.toggle('muted', music.muted || !can || !music.playing)
    b.classList.toggle('off', !can)
    if (b.firstChild) return
    var svg = document.createElementNS(SVG_NS, 'svg')
    svg.setAttribute('viewBox', '0 0 32 32')
    svg.setAttribute('aria-hidden', 'true')
    svg.setAttribute('focusable', 'false')
    var lines = [
      { cls: 'cone', pts: [[5, 13], [10, 13], [16, 7], [16.5, 16], [16, 25], [10, 19], [5, 19], [5, 13]], o: { size: 2.2, thinning: 0.4 } },
      { cls: 'wave', pts: [[20, 12.5], [21.6, 16], [20, 19.5]], o: { size: 2.3, thinning: 0.4, taperStart: 2, taperEnd: 2 } },
      { cls: 'wave', pts: [[23.5, 9], [26.4, 16], [23.5, 23]], o: { size: 2.3, thinning: 0.4, taperStart: 3, taperEnd: 3 } },
      { cls: 'cross', pts: [[3.5, 27.5], [10, 21], [17, 14.5], [24, 8], [28.5, 4]], o: { size: 2.4, thinning: 0.6, taperStart: 2, taperEnd: 10 } },
    ]
    lines.forEach(function (l) {
      var p = document.createElementNS(SVG_NS, 'path')
      p.setAttribute('class', l.cls)
      var outline = inkOutline(l.pts, l.o)
      if (outline) {
        p.setAttribute('d', outlineD(outline, [0.5, 0.5, 31.5, 31.5]))
      } else {
        p.setAttribute('d', pathOf(l.pts))
        p.classList.add('line')
      }
      svg.appendChild(p)
    })
    b.appendChild(svg)
  }

  /** The speaker shows from the first moment, whatever the music can do:
   *  its tooltip says why it's silent. */
  function hushShow() {
    dom.hush.hidden = false
    hushDraw()
  }

  function musicWire() {
    var config = (k2 && k2.config) || {}
    var cfg = config.music
    music.muted = !!prefs.get('music.muted', cfg === false || cfg === 'off')
    hushDraw()
    if (!musicAvailable()) return
    dom.hush.addEventListener('click', function () {
      // A click on a held-back speaker starts the music (inside the click);
      // otherwise it mutes or unmutes.
      if (music.blocked && !music.muted) return
      musicMute(!music.muted)
    })
    // Held back until a gesture: any click, tap or key in the Diary resumes
    // the context right inside the event, never after a promise.
    var nudge = function () {
      if (!music.blocked) return
      music.nudged = true
      musicSync()
    }
    ;['pointerdown', 'mousedown', 'click', 'touchend', 'keydown', 'keyup'].forEach(function (type) {
      document.addEventListener(type, nudge, true)
    })
    window.addEventListener('pagehide', musicClose)
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
    dom.prev.appendChild(curlSvg('prev'))
    dom.next.appendChild(curlSvg('next'))
    grab(dom.prev, -1)
    grab(dom.next, 1)
    dom.pen.addEventListener('submit', function (e) {
      e.preventDefault()
      send()
    })
    dom.ink.addEventListener('input', function () {
      if (dom.ink.value !== '') ghostStop()
      keepDraft()
    })
    // Touching the pen changes nothing on the page: the ghost holds still,
    // and the page never slides away under the pen (Rosson 2026-10-08).
    dom.ink.addEventListener('focus', function () {
      ghostFreeze()
      unslide()
    })
    dom.ink.addEventListener('blur', function () {
      if (dom.ink.value === '') ghostLater(GHOST_IDLE_MS, true)
    })
    ;[dom.desk, dom.book, dom.page].forEach(function (el) {
      el.addEventListener('scroll', unslide)
    })
    document.addEventListener('visibilitychange', sightChanged)
    watchSight()
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
      fx.scroll = dom.sheet.scrollTop
      glideScrolled()
      if (dom.sheet.scrollTop < 24) loadOlder()
    })
    document.addEventListener('keydown', function (e) {
      if (e.metaKey || e.ctrlKey || e.altKey) return
      if ((e.key === 'm' || e.key === 'M') && e.target !== dom.ink && musicAvailable()) {
        // M mutes the music, except while you write (then it's a letter).
        e.preventDefault()
        musicMute(!music.muted)
        return
      }
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
    state.reduced = !!(k2.motion && k2.motion.reduced)
    dom.root.classList.toggle('reduced', state.reduced)
    // You have arrived on this Garden: the paper wakes and the room plays.
    fxInit()
    flameSync()
    musicWire()
    music.arrived = true
    musicSync()

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

  /** Dust turning in the cold beam: a few motes, each on its own slow
   *  path (CSS moves them; nothing here runs per frame). */
  function motes() {
    for (var i = 0; i < 16; i++) {
      var m = make('span', 'mote')
      var k = Math.random()
      m.style.left = (8 + k * 60 + Math.random() * 30).toFixed(1) + '%'
      m.style.top = (k * 85).toFixed(1) + '%'
      m.style.setProperty('--dx', (Math.random() * 30 - 8).toFixed(0) + 'px')
      m.style.setProperty('--dy', (40 + Math.random() * 70).toFixed(0) + 'px')
      m.style.animationDuration = (10 + Math.random() * 12).toFixed(1) + 's'
      m.style.animationDelay = (-Math.random() * 20).toFixed(1) + 's'
      dom.moonbeam.appendChild(m)
    }
  }

  function start() {
    motes()
    hushShow()
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
