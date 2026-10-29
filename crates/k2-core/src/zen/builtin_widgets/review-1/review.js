// The Workbench's review (k2:review@1, 0.46.7). Follows the task the inbox
// picked (`sel` in k2:work-inbox's data, read only) and shows, from git,
// never from the agent's transcript:
//   files     k2.git.snapshot: Branch (committed vs its base) or Uncommitted
//   diff      k2.git.diffFile: the two sides, drawn by the k2-diff kit
//             (K2Diff.render) when it is loaded, else a plain line diff
//   notes     click a line number to note it; Ask for changes sends them to
//             the task's own Thread (k2.git.note); unsent notes are kept in
//             this widget's data (`draft/<ws>/<name>`)
//   commits   k2.git.log: what Land would put on the base
//   tests     k2.compute.jobs: runs on compute for this branch's commits
//   buttons   Land (k2.git.land), Retire / Discard (k2.agents.settle),
//             Terminal (k2.agents.openTerminal); K2 asks the person on its
//             own sheet first, and the widget only hears the outcome
// A task on another computer is read only here (its Thread is in the
// middle column): git reads answer for this computer's agents only.
(function () {
  'use strict'
  var k2 = window.k2
  var WB = window.WB
  var el = WB.el
  var $ = function (id) { return document.getElementById(id) }

  var sel = null
  var scope = 'branch'
  var split = false
  var snap = null
  var file = null
  var notes = []
  var rev = ''
  var busy = false
  var drawn = null

  function can(verb) {
    return !!(k2 && typeof k2.can === 'function' && k2.can(verb))
  }

  function banner(text) {
    $('banner').hidden = !text
    $('banner').textContent = text || ''
  }

  function draftKey() {
    return 'draft/' + String(sel.thread || '').toLowerCase().replace(/[^a-z0-9/_.-]/g, '-')
  }

  // ── A plain line diff (when the kit isn't loaded) ─────────────────────
  function lineDiff(a, b) {
    var x = a ? a.split('\n') : []
    var y = b ? b.split('\n') : []
    if (x.length && x[x.length - 1] === '') x.pop()
    if (y.length && y[y.length - 1] === '') y.pop()
    var out = []
    if (x.length * y.length > 4e6) {
      x.forEach(function (t, i) { out.push({ type: 'del', old: i + 1, new: null, text: t }) })
      y.forEach(function (t, i) { out.push({ type: 'add', old: null, new: i + 1, text: t }) })
      return out
    }
    var n = x.length
    var m = y.length
    var dp = []
    for (var i = 0; i <= n; i++) dp.push(new Uint32Array(m + 1))
    for (i = n - 1; i >= 0; i--) {
      for (var j = m - 1; j >= 0; j--) dp[i][j] = x[i] === y[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1])
    }
    i = 0
    j = 0
    while (i < n || j < m) {
      if (i < n && j < m && x[i] === y[j]) {
        out.push({ type: 'ctx', old: i + 1, new: j + 1, text: x[i] })
        i++
        j++
      } else if (j < m && (i >= n || dp[i][j + 1] >= dp[i + 1][j])) {
        out.push({ type: 'add', old: null, new: j + 1, text: y[j] })
        j++
      } else {
        out.push({ type: 'del', old: i + 1, new: null, text: x[i] })
        i++
      }
    }
    return out
  }

  function drawPlain(box, d) {
    var lines = lineDiff(d.old.exists ? d.old.text || '' : '', d.new.exists ? d.new.text || '' : '')
    var shown = lines.map(function (l, i) {
      var near = false
      for (var k = Math.max(0, i - 3); k <= Math.min(lines.length - 1, i + 3); k++) if (lines[k].type !== 'ctx') near = true
      return near
    })
    var gap = 0
    function flushGap() {
      if (gap) box.appendChild(el('div', 'gap', '⋯ ' + gap + ' unchanged line(s)'))
      gap = 0
    }
    lines.forEach(function (l, i) {
      if (!shown[i]) {
        gap++
        return
      }
      flushGap()
      var row = el('div', 'ln ' + (l.type === 'ctx' ? '' : l.type))
      row.appendChild(el('span', 'n', l.old === null ? '' : l.old))
      var nn = el('span', 'n', l.new === null ? '' : l.new)
      if (l.new !== null) {
        nn.title = 'Add a note on line ' + l.new
        nn.addEventListener('click', function () { askNote(l.new, l.text) })
      }
      row.appendChild(nn)
      row.appendChild(el('span', '', (l.type === 'add' ? '+ ' : l.type === 'del' ? '- ' : '  ') + l.text))
      box.appendChild(row)
    })
    flushGap()
  }

  // ── Reads ─────────────────────────────────────────────────────────────
  function addr() {
    return { address: sel.address }
  }

  async function loadSnapshot(force) {
    if (!sel || sel.remote || !can('git.snapshot')) return
    try {
      var s = await k2.git.snapshot(addr())
      if (!force && s && s.rev === rev) return
      rev = s ? s.rev : ''
      snap = s
      drawMeta()
      drawFiles()
      if (file) await showFile(file)
    } catch (e) {
      snap = null
      $('files').textContent = ''
      $('diff').textContent = ''
      $('meta').textContent = e && e.code === 'not_a_repo' ? 'This agent’s folder isn’t a git repository.' : 'Git: ' + WB.errText(e)
    }
  }

  function scoped() {
    var all = snap && snap.files ? snap.files : []
    return all.filter(function (f) { return scope === 'branch' ? f.group === 'base' : f.group !== 'base' })
  }

  function drawMeta() {
    var m = $('meta')
    m.textContent = ''
    if (!snap) return
    m.appendChild(el('span', 'mono', (snap.branch || 'detached') + (snap.base ? ' → ' + snap.base.ref : '')))
    if (snap.base) m.appendChild(el('span', '', snap.base.ahead + ' ahead · ' + snap.base.behind + ' behind'))
    var c = snap.counts || {}
    m.appendChild(el('span', '', (c.total - (c.base || 0)) + ' uncommitted'))
    if (snap.state && snap.state !== 'clean') m.appendChild(el('span', 'del', snap.state))
  }

  function drawFiles() {
    var box = $('files')
    box.textContent = ''
    var files = scoped()
    if (!files.length) {
      box.appendChild(el('p', 'empty', scope === 'branch' ? 'No commits on this branch yet.' : 'Nothing uncommitted.'))
      $('diff').textContent = ''
      return
    }
    if (window.K2Diff && typeof window.K2Diff.tree === 'function') {
      if (drawn && drawn.tree) drawn.tree.destroy()
      drawn = drawn || {}
      drawn.tree = window.K2Diff.tree(box, files, { selected: file, onSelect: function (f) { showFile(f) } })
      return
    }
    files.forEach(function (f) {
      var row = el('div', 'file' + (f.file === file ? ' selected' : ''))
      row.appendChild(el('span', 'st', (f.status || '?').charAt(0).toUpperCase()))
      row.appendChild(el('span', 'fn', f.file))
      if (f.additions !== null && f.additions !== undefined) row.appendChild(el('span', 'add', '+' + f.additions))
      if (f.deletions !== null && f.deletions !== undefined) row.appendChild(el('span', 'del', '−' + f.deletions))
      row.addEventListener('click', function () { showFile(f.file) })
      box.appendChild(row)
    })
  }

  async function showFile(name) {
    file = name
    drawFiles()
    var box = $('diff')
    var f = scoped().find(function (x) { return x.file === name })
    if (!f) {
      box.textContent = ''
      return
    }
    try {
      var args = { address: sel.address, file: f.file }
      if (f.oldFile) args.oldFile = f.oldFile
      if (scope === 'branch' && snap && snap.base) args.base = snap.base.ref
      var d = await k2.git.diffFile(args)
      if (file !== name) return
      if (drawn && drawn.diff) drawn.diff.destroy()
      box.textContent = ''
      if (d.binary) return void box.appendChild(el('p', 'empty', 'A binary file: no text diff.'))
      if (d.tooLarge) return void box.appendChild(el('p', 'empty', 'Too large to show here. Open it in K2’s Changes tab.'))
      if (window.K2Diff && typeof window.K2Diff.render === 'function') {
        drawn = drawn || {}
        drawn.diff = window.K2Diff.render(box, d, {
          view: split ? 'split' : 'unified',
          highlight: true,
          onLine: function (info) { if (info && info.side !== 'old') askNote(info.line, info.text) },
        })
      } else {
        drawPlain(box, d)
      }
    } catch (e) {
      box.textContent = ''
      box.appendChild(el('p', 'empty', WB.errText(e)))
    }
  }

  async function loadCommits() {
    var ul = $('commits')
    ul.textContent = ''
    if (!can('git.log') || sel.remote) return void ($('commitsPanel').hidden = true)
    try {
      var log = await k2.git.log(addr())
      $('commitsPanel').hidden = false
      $('commitsTitle').textContent = 'Commits (' + log.ahead + ' to land on ' + (log.base || 'base') + ')'
      log.commits.slice(0, 20).forEach(function (c) {
        var li = el('li')
        li.appendChild(el('span', 'mono', c.short + ' '))
        li.appendChild(document.createTextNode(c.subject))
        ul.appendChild(li)
      })
      $('btnLand').disabled = log.ahead === 0
    } catch (e) {
      $('commitsPanel').hidden = true
    }
  }

  async function loadTests() {
    var ul = $('tests')
    ul.textContent = ''
    if (!can('compute.jobs') || sel.remote) return void ($('testsPanel').hidden = true)
    try {
      var jobs = await k2.compute.jobs({ address: sel.address, limit: 5 })
      $('testsPanel').hidden = jobs.length === 0
      jobs.forEach(function (j) {
        var ok = j.state === 'succeeded' || j.exitCode === 0
        var li = el('li', ok ? 'add' : j.state === 'running' || j.state === 'queued' ? '' : 'del')
        li.textContent =
          (ok ? '✓ ' : j.state === 'running' ? '◐ ' : j.state === 'queued' ? '… ' : '✗ ') +
          j.state + ' on ' + (j.node || '?') + (j.commit ? ' @' + j.commit : '') + (j.secs !== null ? ' · ' + j.secs + 's' : '') + ' · ' + j.id
        li.title = j.command
        ul.appendChild(li)
      })
    } catch (e) {
      $('testsPanel').hidden = true
    }
  }

  // ── Notes ─────────────────────────────────────────────────────────────
  var pendingLine = null
  function askNote(line) {
    if (!can('git.note') || !file) return
    pendingLine = line
    $('noteTitle').textContent = 'Note on ' + file + ':' + line
    $('noteBody').value = ''
    $('noteDlg').hidden = false
    $('noteBody').focus()
  }

  function drawNotes() {
    var box = $('notes')
    box.textContent = ''
    $('notesPanel').hidden = notes.length === 0
    notes.forEach(function (n, i) {
      var row = el('div', 'n')
      row.appendChild(el('span', 'mono', n.file + (n.line ? ':' + n.line : '')))
      row.appendChild(el('span', '', n.body))
      var x = el('button', 'btn small', 'Remove')
      x.type = 'button'
      x.addEventListener('click', function () {
        notes.splice(i, 1)
        saveDraft()
      })
      row.appendChild(x)
      box.appendChild(row)
    })
    $('btnAsk').textContent = notes.length ? 'Ask for changes (' + notes.length + ')' : 'Ask for changes'
    $('btnAsk').disabled = notes.length === 0
  }

  async function saveDraft() {
    drawNotes()
    try {
      if (notes.length) await k2.store.put(draftKey(), { notes: notes })
      else await k2.store.delete(draftKey())
    } catch (e) {
      // The notes stay on screen; they just aren't kept.
    }
  }

  $('noteForm').addEventListener('submit', function (ev) {
    ev.preventDefault()
    var body = $('noteBody').value.trim()
    $('noteDlg').hidden = true
    if (!body || !file) return
    notes.push({ file: file, line: pendingLine, body: body })
    saveDraft()
  })
  $('noteCancel').addEventListener('click', function () { $('noteDlg').hidden = true })

  // ── Buttons ───────────────────────────────────────────────────────────
  async function act(fn, label) {
    if (busy || !sel) return
    busy = true
    banner(label + '…')
    try {
      await fn()
    } catch (e) {
      banner(label + ': ' + WB.errText(e))
      busy = false
      return
    }
    busy = false
    await reload(true)
  }

  $('btnLand').addEventListener('click', function () {
    act(async function () {
      var r = await k2.git.land(addr())
      if (r.cancelled) banner('')
      else if (r.landed) banner('Landed ' + r.commits + ' commit(s)' + (r.head ? ' (' + r.head + ')' : '') + '.')
      else if (r.conflict && r.conflict.length) banner('Couldn’t land: conflicts in ' + r.conflict.join(', ') + '. The agent was told.')
    }, 'Land')
  })
  function settle(discard) {
    act(async function () {
      var r = await k2.agents.settle({ address: sel.address, discard: discard })
      banner(r.cancelled ? '' : r.settled ? (discard ? 'Discarded.' : 'Retired.') : '')
    }, discard ? 'Discard' : 'Retire')
  }
  $('btnRetire').addEventListener('click', function () { settle(false) })
  $('btnDiscard').addEventListener('click', function () { settle(true) })
  $('btnAsk').addEventListener('click', function () {
    act(async function () {
      var r = await k2.git.note({ address: sel.address, notes: notes })
      notes = []
      await saveDraft()
      banner('Sent ' + r.sent + ' note(s) to ' + (sel.name || sel.thread) + '.')
    }, 'Ask for changes')
  })
  $('btnTerm').addEventListener('click', function () {
    act(function () { return k2.agents.openTerminal(sel.address) }, 'Terminal')
  })
  $('btnRefresh').addEventListener('click', function () { reload(true) })
  function setScope(s) {
    scope = s
    file = null
    $('tabBranch').setAttribute('aria-selected', s === 'branch' ? 'true' : 'false')
    $('tabLocal').setAttribute('aria-selected', s === 'local' ? 'true' : 'false')
    drawFiles()
    $('diff').textContent = ''
  }
  $('tabBranch').addEventListener('click', function () { setScope('branch') })
  $('tabLocal').addEventListener('click', function () { setScope('local') })
  $('btnView').addEventListener('click', function () {
    split = !split
    $('btnView').textContent = split ? 'Unified' : 'Side by side'
    if (file) showFile(file)
  })

  function drawButtons() {
    var local = sel && !sel.remote && WB.isSidecar(sel.address)
    $('actions').hidden = !sel || sel.remote
    $('btnLand').hidden = !local || !can('git.land')
    $('btnRetire').hidden = !local || !can('agents.settle')
    $('btnDiscard').hidden = !local || !can('agents.settle')
    $('btnAsk').hidden = !local || !can('git.note')
    $('btnTerm').hidden = !sel || sel.remote || !can('agents.openTerminal')
    $('btnView').hidden = !(window.K2Diff && typeof window.K2Diff.render === 'function')
  }

  // ── Following the inbox ───────────────────────────────────────────────
  async function reload(force) {
    if (!sel) return
    drawButtons()
    if (sel.remote) {
      $('meta').textContent = 'On another computer: its Thread is in the middle column. Review runs on the computer the agent runs on.'
      $('files').textContent = ''
      $('diff').textContent = ''
      $('commitsPanel').hidden = true
      $('testsPanel').hidden = true
      return
    }
    await Promise.all([loadSnapshot(force), loadCommits(), loadTests()])
  }

  async function follow(value) {
    var next = value && value.address ? value : null
    if (next && sel && next.address === sel.address) return
    sel = next
    rev = ''
    file = null
    notes = []
    $('empty').hidden = !!sel
    $('body').hidden = !sel
    $('title').textContent = sel ? sel.name || sel.thread : 'Review'
    if (!sel) {
      $('actions').hidden = true
      return
    }
    try {
      var d = await k2.store.get(draftKey())
      notes = d && d.value && Array.isArray(d.value.notes) ? d.value.notes : []
    } catch (e) {
      notes = []
    }
    drawNotes()
    await reload(true)
  }

  async function start() {
    try {
      var s = await k2.store.get('sel', { from: 'k2:work-inbox' })
      await follow(s ? s.value : null)
      k2.store.subscribe(
        { prefix: 'sel', from: 'k2:work-inbox' },
        function (changes) {
          var list = Array.isArray(changes) ? changes : changes && changes.items ? changes.items : []
          var last = list.filter(function (c) { return c.key === 'sel' }).pop()
          if (last) follow(last.value)
        },
        function (e) { banner('Following the inbox stopped: ' + WB.errText(e)) }
      )
    } catch (e) {
      banner('Can’t follow the inbox here: ' + WB.errText(e))
    }
    k2.ready()
    setInterval(function () {
      if (sel && !sel.remote && document.visibilityState !== 'hidden') loadSnapshot(false)
    }, 5000)
  }
  start()
})()
