/*! k2-diff 1.0.0 (FSL-1.1-Apache-2.0), K2's widget kit: line diffs, a diff view and a changed-files tree. Copyright 2025-2026 Alakazam Labs. */
// K2Diff: show what changed in a file inside a Garden (the Workbench's
// Review column, catalog v40 git.diffFile).
//
//   requires.libs: ["k2-diff@1"]   (add "highlight.js@11" for colours)
//
//   const d = await k2.git.diffFile(row.address, {file: 'src/app.ts', base: 'main'})
//   const view = K2Diff.render(el, d, {view: 'split', onLine: (i) => addNote(i)})
//   const tree = K2Diff.tree(listEl, snapshot.files, {onSelect: (file) => show(file)})
//   K2Diff.lines(oldText, newText)  → [{type: 'ctx'|'add'|'del', old, new, text}]
//   K2Diff.hunks(lines, {context: 3}), K2Diff.stats(lines) → {additions, deletions}
//
// Everything is drawn with textContent and K2's classes (never innerHTML
// from data), in the Garden's colours (--zen-*). A diff too big to compare
// line by line in time shows as the whole file replaced (lines.whole).
;(function () {
  'use strict'
  if (window.K2Diff) return
  var d = document

  // ── the line diff ─────────────────────────────────────────────────────

  /** Myers' edit distance is capped: past this many edits the middle of
   *  the file shows as replaced (time and memory stay bounded). */
  var MAX_EDITS = 1000
  var HIGHLIGHT_MAX_LINES = 5000

  function split(text) {
    if (text === null || text === undefined || text === '') return { lines: [], eofNewline: true }
    var s = String(text).replace(/\r\n/g, '\n')
    var lines = s.split('\n')
    var eofNewline = lines[lines.length - 1] === ''
    if (eofNewline) lines.pop()
    return { lines: lines, eofNewline: eofNewline }
  }

  function keysOf(sp, ignoreWs) {
    var out = new Array(sp.lines.length)
    for (var i = 0; i < sp.lines.length; i++) {
      var t = sp.lines[i]
      out[i] = ignoreWs ? t.replace(/\s+/g, ' ').trim() : t
    }
    // A last line without its newline is not the same line as one with it.
    if (!sp.eofNewline && out.length) out[out.length - 1] += '\u0000eof'
    return out
  }

  /** Myers O(ND): ops over a[lo..] / b[lo..], or null past MAX_EDITS. */
  function myers(a, b) {
    var n = a.length
    var m = b.length
    var max = n + m
    var lim = Math.min(max, MAX_EDITS)
    var off = lim + 1
    var v = new Int32Array(2 * lim + 3)
    var trace = []
    for (var e = 0; e <= lim; e++) {
      trace.push(v.slice(off - e - 1, off + e + 2))
      for (var k = -e; k <= e; k += 2) {
        var x = k === -e || (k !== e && v[off + k - 1] < v[off + k + 1]) ? v[off + k + 1] : v[off + k - 1] + 1
        var y = x - k
        while (x < n && y < m && a[x] === b[y]) {
          x++
          y++
        }
        v[off + k] = x
        if (x >= n && y >= m) return back(trace, e, n, m)
      }
    }
    return null
  }

  function back(trace, last, n, m) {
    var ops = []
    var x = n
    var y = m
    for (var e = last; e >= 0; e--) {
      var vs = trace[e]
      var get = function (k) {
        return vs[k + e + 1]
      }
      var k = x - y
      var pk = k === -e || (k !== e && get(k - 1) < get(k + 1)) ? k + 1 : k - 1
      var px = get(pk)
      var py = px - pk
      while (x > px && y > py) {
        ops.push(0)
        x--
        y--
      }
      if (e > 0) ops.push(x === px ? 1 : -1)
      x = px
      y = py
    }
    return ops.reverse()
  }

  /** The lines of a diff: ctx / del / add with 1-based old and new numbers. */
  function lines(oldText, newText, opts) {
    var o = opts || {}
    var A = split(oldText)
    var B = split(newText)
    var a = keysOf(A, !!o.ignoreWhitespace)
    var b = keysOf(B, !!o.ignoreWhitespace)
    var pre = 0
    while (pre < a.length && pre < b.length && a[pre] === b[pre]) pre++
    var suf = 0
    while (suf < a.length - pre && suf < b.length - pre && a[a.length - 1 - suf] === b[b.length - 1 - suf]) suf++
    var midA = a.slice(pre, a.length - suf)
    var midB = b.slice(pre, b.length - suf)
    var ops = midA.length && midB.length ? myers(midA, midB) : null
    var whole = false
    if (!ops) {
      if (midA.length && midB.length) whole = true
      ops = []
      for (var i = 0; i < midA.length; i++) ops.push(-1)
      for (var j = 0; j < midB.length; j++) ops.push(1)
    }
    var out = []
    var oi = 0
    var ni = 0
    var push = function (type) {
      var row = { type: type, old: null, new: null, text: '' }
      if (type !== 'add') row.old = oi + 1
      if (type !== 'del') row.new = ni + 1
      row.text = type === 'del' ? A.lines[oi] : B.lines[ni]
      if (type === 'del' && oi === A.lines.length - 1 && !A.eofNewline) row.noNewline = true
      if (type !== 'del' && ni === B.lines.length - 1 && !B.eofNewline) row.noNewline = true
      if (type !== 'add') oi++
      if (type !== 'del') ni++
      out.push(row)
    }
    for (var p = 0; p < pre; p++) push('ctx')
    for (var q = 0; q < ops.length; q++) push(ops[q] === 0 ? 'ctx' : ops[q] < 0 ? 'del' : 'add')
    for (var s = 0; s < suf; s++) push('ctx')
    if (whole) out.whole = true
    return out
  }

  function stats(ls) {
    var r = { additions: 0, deletions: 0 }
    for (var i = 0; i < ls.length; i++) {
      if (ls[i].type === 'add') r.additions++
      else if (ls[i].type === 'del') r.deletions++
    }
    return r
  }

  /** Which lines show: every change and `ctx` lines around it. */
  function visible(ls, ctx) {
    var show = new Array(ls.length)
    for (var i = 0; i < ls.length; i++) {
      if (ls[i].type === 'ctx') continue
      for (var j = Math.max(0, i - ctx); j <= Math.min(ls.length - 1, i + ctx); j++) show[j] = true
    }
    return show
  }

  /** Unified hunks: each change with `context` lines around it (nearby
   *  changes share a hunk). */
  function hunks(ls, opts) {
    var ctx = opts && typeof opts.context === 'number' && opts.context >= 0 ? Math.floor(opts.context) : 3
    var show = visible(ls, ctx)
    var out = []
    var i = 0
    while (i < ls.length) {
      if (!show[i]) {
        i++
        continue
      }
      var h = { oldStart: 0, oldLines: 0, newStart: 0, newLines: 0, lines: [] }
      while (i < ls.length && show[i]) {
        var l = ls[i++]
        h.lines.push(l)
        if (l.old !== null) {
          if (!h.oldLines) h.oldStart = l.old
          h.oldLines++
        }
        if (l.new !== null) {
          if (!h.newLines) h.newStart = l.new
          h.newLines++
        }
      }
      out.push(h)
    }
    return out
  }

  // ── styles ────────────────────────────────────────────────────────────

  var CSS = [
    '.k2d{font-family:var(--zen-font-mono,ui-monospace,SFMono-Regular,Menlo,monospace);font-size:12px;line-height:1.5;color:var(--zen-text,inherit);min-width:0}',
    '.k2d-head{display:flex;gap:8px;align-items:baseline;padding:6px 8px;border-bottom:1px solid var(--zen-border,rgba(127,127,127,.3));font-family:var(--zen-font,inherit)}',
    '.k2d-file{font-weight:600;overflow-wrap:anywhere}.k2d-kind{color:var(--zen-text-muted,#888)}',
    '.k2d-plus{color:var(--zen-diff-add-text,#1a7f37)}.k2d-minus{color:var(--zen-diff-del-text,#cf222e)}',
    '.k2d-note-msg{padding:12px 8px;color:var(--zen-text-muted,#888);font-family:var(--zen-font,inherit)}',
    '.k2d-body{overflow:auto}',
    '.k2d-row{display:grid;grid-template-columns:3.5em 3.5em 1.2em minmax(0,1fr);align-items:start;position:relative}',
    '.k2d-split .k2d-row{grid-template-columns:3.5em 1.2em minmax(0,1fr) 3.5em 1.2em minmax(0,1fr)}',
    '.k2d-n{color:var(--zen-text-muted,#888);text-align:right;padding-right:6px;user-select:none}',
    '.k2d-s{user-select:none;text-align:center;color:var(--zen-text-muted,#888)}',
    '.k2d-c{white-space:pre;overflow-wrap:normal;min-width:0;padding-right:8px}',
    '.k2d-wrap .k2d-c{white-space:pre-wrap;overflow-wrap:anywhere}',
    '.k2d-add{background:var(--zen-diff-add,rgba(46,160,67,.15))}',
    '.k2d-del{background:var(--zen-diff-del,rgba(248,81,73,.15))}',
    '.k2d-split .k2d-row>.k2d-add{background:var(--zen-diff-add,rgba(46,160,67,.15))}',
    '.k2d-split .k2d-row>.k2d-del{background:var(--zen-diff-del,rgba(248,81,73,.15))}',
    '.k2d-empty{background:var(--zen-surface-raised,rgba(127,127,127,.08))}',
    '.k2d-eof{color:var(--zen-text-muted,#888);font-style:italic;padding-left:7.2em}',
    '.k2d-gap{display:block;width:100%;text-align:left;padding:2px 8px 2px 7.2em;border:0;border-block:1px solid var(--zen-border,rgba(127,127,127,.25));background:var(--zen-surface-raised,rgba(127,127,127,.08));color:var(--zen-text-muted,#888);font:inherit;cursor:pointer}',
    '.k2d-gap:hover,.k2d-gap:focus-visible{color:var(--zen-text,inherit)}',
    '.k2d-addnote{position:absolute;left:0;top:0;width:1.4em;height:1.4em;padding:0;border:0;border-radius:4px;background:var(--zen-accent,#0969da);color:var(--zen-on-accent,#fff);font:inherit;line-height:1.4em;cursor:pointer;opacity:0}',
    '.k2d-row:hover>.k2d-addnote,.k2d-addnote:focus-visible{opacity:1}',
    '.k2d-tree{font-family:var(--zen-font,inherit);font-size:12px;color:var(--zen-text,inherit)}',
    '.k2d-t{display:flex;gap:6px;align-items:center;width:100%;padding:2px 6px;border:0;border-radius:var(--zen-radius-sm,4px);background:none;color:inherit;font:inherit;text-align:left;cursor:pointer;min-width:0}',
    '.k2d-t:hover{background:var(--zen-hover,rgba(127,127,127,.12))}',
    '.k2d-t[aria-selected="true"]{background:var(--zen-selected,rgba(9,105,218,.16))}',
    '.k2d-tn{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}',
    '.k2d-tst{width:1.2em;text-align:center;font-weight:600;color:var(--zen-text-muted,#888)}',
    '.k2d-tst-A,.k2d-tst-U{color:var(--zen-diff-add-text,#1a7f37)}.k2d-tst-D{color:var(--zen-diff-del-text,#cf222e)}.k2d-tst-M,.k2d-tst-R{color:var(--zen-warn,#9a6700)}',
    '.k2d-tc{font-family:var(--zen-font-mono,ui-monospace,monospace);font-size:11px;white-space:nowrap}',
    '.k2d-tdir>.k2d-tkids{padding-left:12px}.k2d-tdir.k2d-closed>.k2d-tkids{display:none}',
  ].join('\n')

  var styled = false
  function addStyle() {
    if (styled) return
    styled = true
    var s = d.createElement('style')
    s.setAttribute('data-k2-lib', 'k2-diff')
    s.textContent = CSS
    ;(d.head || d.documentElement).appendChild(s)
  }

  function el(tag, cls, text) {
    var e = d.createElement(tag)
    if (cls) e.className = cls
    if (text !== undefined && text !== null) e.textContent = String(text)
    return e
  }

  function clear(node) {
    while (node.firstChild) node.removeChild(node.firstChild)
  }

  // ── highlighting (optional: highlight.js@11 when the widget loads it) ──

  var LANG = { tsx: 'typescript', jsx: 'javascript', mjs: 'javascript', cjs: 'javascript', h: 'c', hpp: 'cpp', yml: 'yaml', htm: 'xml', html: 'xml', svg: 'xml', vue: 'xml', sh: 'bash', zsh: 'bash' }
  var NAMES = { dockerfile: 'dockerfile', makefile: 'makefile' }

  function language(file) {
    var h = window.hljs
    if (!h || typeof h.highlight !== 'function' || typeof h.getLanguage !== 'function') return null
    var base = String(file || '').split('/').pop().toLowerCase()
    var dot = base.lastIndexOf('.')
    var name = NAMES[base] || (dot >= 0 ? base.slice(dot + 1) : '')
    name = LANG[name] || name
    return name && h.getLanguage(name) ? name : null
  }

  /** highlight.js's token tree, cut into lines: per line, segments of
   *  {text, cls}. Never its HTML string. Null when it can't. */
  function tokenLines(text, lang) {
    try {
      var r = window.hljs.highlight(text, { language: lang, ignoreIllegals: true })
      var root = r && r._emitter && r._emitter.rootNode
      if (!root || !root.children) return null
      var out = [[]]
      var walk = function (node, cls) {
        if (typeof node === 'string') {
          var parts = node.split('\n')
          for (var i = 0; i < parts.length; i++) {
            if (i > 0) out.push([])
            if (parts[i]) out[out.length - 1].push({ text: parts[i], cls: cls })
          }
          return
        }
        var scope = node.scope || node.kind
        var c = cls
        if (scope && typeof scope === 'string' && scope.indexOf('language:') !== 0) {
          var ps = scope.split('.')
          var mine = 'hljs-' + ps[0]
          for (var j = 1; j < ps.length; j++) mine += ' ' + ps[j] + '_'.repeat(j)
          c = cls ? cls + ' ' + mine : mine
        }
        for (var k = 0; k < (node.children || []).length; k++) walk(node.children[k], c)
      }
      walk(root, '')
      return out
    } catch (e) {
      return null
    }
  }

  // ── the diff view ─────────────────────────────────────────────────────

  function side(v) {
    if (!v || typeof v !== 'object') return { exists: false, text: '' }
    return { exists: v.exists !== false, text: typeof v.text === 'string' ? v.text : '' }
  }

  function render(host, data, opts) {
    addStyle()
    var o = opts || {}
    var dd = data || {}
    var ctxN = typeof o.context === 'number' && o.context >= 0 ? Math.floor(o.context) : 3
    var isSplit = o.view === 'split'
    var file = String(dd.file || '')
    clear(host)
    var root = el('div', 'k2d' + (isSplit ? ' k2d-split' : '') + (o.wrap ? ' k2d-wrap' : ''))
    host.appendChild(root)
    var oldS = side(dd.old)
    var newS = side(dd.new)
    var head = el('div', 'k2d-head')
    head.appendChild(el('span', 'k2d-file', dd.oldFile && dd.oldFile !== file ? dd.oldFile + ' → ' + file : file))
    root.appendChild(head)
    var msg = function (t) {
      root.appendChild(el('div', 'k2d-note-msg', t))
      return { destroy: destroy, lines: [] }
    }
    function destroy() {
      clear(host)
    }
    if (dd.binary) return msg('Binary file: there is no text to compare.')
    if (dd.tooLarge) return msg('This file is too large to show here. Open it in K2 to see the change.')
    if (!oldS.exists && !newS.exists) return msg('This file isn’t there on either side.')
    if (!oldS.exists) head.appendChild(el('span', 'k2d-kind', 'New file'))
    if (!newS.exists) head.appendChild(el('span', 'k2d-kind', 'Deleted'))
    var ls = lines(oldS.exists ? oldS.text : '', newS.exists ? newS.text : '', { ignoreWhitespace: !!o.ignoreWhitespace })
    var st = stats(ls)
    head.appendChild(el('span', 'k2d-plus', '+' + st.additions))
    head.appendChild(el('span', 'k2d-minus', '−' + st.deletions))
    if (ls.whole) head.appendChild(el('span', 'k2d-kind', 'Shown as replaced: too many changes to compare line by line'))
    if (!st.additions && !st.deletions) {
      return msg(o.ignoreWhitespace && oldS.text !== newS.text ? 'Only whitespace changed.' : 'No changes.')
    }
    var lang = o.highlight === false ? null : language(file)
    var tok = { old: null, new: null }
    if (lang) {
      var oc = split_count(oldS.text)
      var nc = split_count(newS.text)
      if (oc <= HIGHLIGHT_MAX_LINES && nc <= HIGHLIGHT_MAX_LINES) {
        tok.old = oldS.exists ? tokenLines(oldS.text.replace(/\r\n/g, '\n'), lang) : null
        tok.new = newS.exists ? tokenLines(newS.text.replace(/\r\n/g, '\n'), lang) : null
      }
    }
    var body = el('div', 'k2d-body')
    root.appendChild(body)

    function code(l, which) {
      var c = el('span', 'k2d-c')
      var n = which === 'old' ? l.old : l.new
      var segs = tok[which] && n ? tok[which][n - 1] : null
      if (segs && segs.map(function (s) { return s.text }).join('') === l.text) {
        for (var i = 0; i < segs.length; i++) {
          if (segs[i].cls) c.appendChild(el('span', segs[i].cls, segs[i].text))
          else c.appendChild(d.createTextNode(segs[i].text))
        }
      } else c.textContent = l.text
      return c
    }

    function noteButton(row, l) {
      if (typeof o.onLine !== 'function') return
      var which = l.type === 'del' ? 'old' : 'new'
      var n = which === 'old' ? l.old : l.new
      var b = el('button', 'k2d-addnote', '+')
      b.type = 'button'
      b.setAttribute('aria-label', 'Add a note on line ' + n)
      b.addEventListener('click', function () {
        o.onLine({ file: file, side: which, line: n, text: l.text })
      })
      row.appendChild(b)
    }

    var SIGN = { ctx: ' ', add: '+', del: '−' }
    function unifiedRow(l) {
      var r = el('div', 'k2d-row k2d-' + l.type)
      r.appendChild(el('span', 'k2d-n', l.old))
      r.appendChild(el('span', 'k2d-n', l.new))
      r.appendChild(el('span', 'k2d-s', SIGN[l.type]))
      r.appendChild(code(l, l.type === 'del' ? 'old' : 'new'))
      noteButton(r, l)
      return r
    }
    function half(r, l, which) {
      if (!l) {
        r.appendChild(el('span', 'k2d-n k2d-empty'))
        r.appendChild(el('span', 'k2d-s k2d-empty'))
        r.appendChild(el('span', 'k2d-c k2d-empty'))
        return
      }
      var cls = l.type === 'ctx' ? '' : ' k2d-' + l.type
      r.appendChild(el('span', 'k2d-n' + cls, which === 'old' ? l.old : l.new))
      r.appendChild(el('span', 'k2d-s' + cls, SIGN[l.type]))
      var c = code(l, which)
      if (cls) c.className += cls
      r.appendChild(c)
    }
    function splitRows(chunk, into) {
      var i = 0
      while (i < chunk.length) {
        if (chunk[i].type === 'ctx') {
          var r = el('div', 'k2d-row k2d-ctx')
          half(r, chunk[i], 'old')
          half(r, chunk[i], 'new')
          noteButton(r, chunk[i])
          into.appendChild(r)
          i++
          continue
        }
        var dels = []
        var adds = []
        while (i < chunk.length && chunk[i].type === 'del') dels.push(chunk[i++])
        while (i < chunk.length && chunk[i].type === 'add') adds.push(chunk[i++])
        for (var j = 0; j < Math.max(dels.length, adds.length); j++) {
          var rr = el('div', 'k2d-row')
          half(rr, dels[j], 'old')
          half(rr, adds[j], 'new')
          noteButton(rr, adds[j] || dels[j])
          into.appendChild(rr)
        }
      }
    }
    function draw(chunk, into) {
      if (isSplit) splitRows(chunk, into)
      else for (var i = 0; i < chunk.length; i++) into.appendChild(unifiedRow(chunk[i]))
      var last = chunk[chunk.length - 1]
      if (last && last.noNewline) into.appendChild(el('div', 'k2d-eof', 'No newline at end of file'))
    }

    // Visible lines: every change, `ctxN` lines around it; longer unchanged
    // runs fold behind an "Expand N lines" button.
    var show = visible(ls, ctxN)
    var at = 0
    while (at < ls.length) {
      var start = at
      var vis = !!show[at]
      while (at < ls.length && !!show[at] === vis) at++
      var chunk = ls.slice(start, at)
      if (vis) draw(chunk, body)
      else gap(chunk)
    }
    function gap(chunk) {
      var b = el('button', 'k2d-gap', 'Expand ' + chunk.length + (chunk.length === 1 ? ' line' : ' lines'))
      b.type = 'button'
      b.addEventListener('click', function () {
        var f = d.createDocumentFragment()
        draw(chunk, f)
        b.parentNode.replaceChild(f, b)
      })
      body.appendChild(b)
    }
    return { destroy: destroy, lines: ls }
  }

  function split_count(text) {
    return split(text).lines.length
  }

  // ── the changed-files tree ────────────────────────────────────────────

  var LETTER = { added: 'A', modified: 'M', deleted: 'D', renamed: 'R', untracked: 'U', copied: 'C', conflicted: '!', typechange: 'T' }

  function tree(host, files, opts) {
    addStyle()
    var o = opts || {}
    clear(host)
    var root = el('div', 'k2d-tree')
    root.setAttribute('role', 'tree')
    host.appendChild(root)
    var buttons = {}
    var top = { dirs: {}, files: [] }
    var list = Array.isArray(files) ? files : []
    for (var i = 0; i < list.length; i++) {
      var f = list[i]
      if (!f || typeof f.file !== 'string') continue
      var parts = f.file.split('/')
      var node = top
      for (var j = 0; j < parts.length - 1; j++) {
        node = node.dirs[parts[j]] || (node.dirs[parts[j]] = { dirs: {}, files: [] })
      }
      node.files.push({ name: parts[parts.length - 1], f: f })
    }
    function select(file) {
      for (var k in buttons) buttons[k].setAttribute('aria-selected', k === file ? 'true' : 'false')
    }
    function build(node, into) {
      var names = Object.keys(node.dirs).sort()
      for (var a = 0; a < names.length; a++) {
        // One-child folder chains read as one row (src/lib).
        var label = names[a]
        var sub = node.dirs[names[a]]
        while (!sub.files.length && Object.keys(sub.dirs).length === 1) {
          var only = Object.keys(sub.dirs)[0]
          label += '/' + only
          sub = sub.dirs[only]
        }
        var wrap = el('div', 'k2d-tdir')
        var b = el('button', 'k2d-t')
        b.type = 'button'
        b.setAttribute('role', 'treeitem')
        b.setAttribute('aria-expanded', 'true')
        b.appendChild(el('span', 'k2d-tst', '▾'))
        b.appendChild(el('span', 'k2d-tn', label))
        var kids = el('div', 'k2d-tkids')
        kids.setAttribute('role', 'group')
        ;(function (b, wrap) {
          b.addEventListener('click', function () {
            var closed = wrap.classList.toggle('k2d-closed')
            b.setAttribute('aria-expanded', closed ? 'false' : 'true')
            b.firstChild.textContent = closed ? '▸' : '▾'
          })
        })(b, wrap)
        wrap.appendChild(b)
        wrap.appendChild(kids)
        into.appendChild(wrap)
        build(sub, kids)
      }
      node.files.sort(function (x, y) {
        return x.name < y.name ? -1 : x.name > y.name ? 1 : 0
      })
      for (var c = 0; c < node.files.length; c++) {
        var it = node.files[c]
        var fb = el('button', 'k2d-t')
        fb.type = 'button'
        fb.setAttribute('role', 'treeitem')
        fb.setAttribute('aria-selected', 'false')
        fb.title = it.f.file
        var letter = LETTER[it.f.status] || (it.f.status ? String(it.f.status).charAt(0).toUpperCase() : '·')
        fb.appendChild(el('span', 'k2d-tst k2d-tst-' + letter, letter))
        fb.appendChild(el('span', 'k2d-tn', it.name))
        var counts = el('span', 'k2d-tc')
        if (typeof it.f.additions === 'number') counts.appendChild(el('span', 'k2d-plus', '+' + it.f.additions + ' '))
        if (typeof it.f.deletions === 'number') counts.appendChild(el('span', 'k2d-minus', '−' + it.f.deletions))
        fb.appendChild(counts)
        ;(function (file) {
          fb.addEventListener('click', function () {
            select(file)
            if (typeof o.onSelect === 'function') o.onSelect(file)
          })
        })(it.f.file)
        buttons[it.f.file] = fb
        into.appendChild(fb)
      }
    }
    build(top, root)
    if (o.selected) select(o.selected)
    return {
      select: select,
      destroy: function () {
        clear(host)
      },
    }
  }

  window.K2Diff = Object.freeze({ version: '1.0.0', lines: lines, hunks: hunks, stats: stats, render: render, tree: tree })
})()
