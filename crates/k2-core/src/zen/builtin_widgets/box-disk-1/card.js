// k2:box-disk@1 (0.46.4, disk guard Slice B): this computer's disks for a
// Garden. One read a minute of k2.app.resources() (garden-local, read
// only): each disk's use as a bar with the guard's warn / refuse /
// critical lines marked on it, its level, the biggest users and the agents
// behind them (names and handles, never a folder path), and what K2's own
// daily sweep clears. Nothing here changes anything: no buttons.
// Config: title? (default "Box disk").
(function () {
  'use strict'
  var k2 = window.k2
  var root = document.getElementById('card')
  var EVERY_MS = 60 * 1000
  var readied = false
  var last = null

  function el(tag, cls, text) {
    var e = document.createElement(tag)
    if (cls) e.className = cls
    if (text !== undefined && text !== null) e.textContent = String(text)
    return e
  }

  function clear(n) {
    while (n.firstChild) n.removeChild(n.firstChild)
  }

  // Decimal sizes, like k2 app resources: 53 GB, 1.4 GB, 50 MB.
  function size(b) {
    if (typeof b !== 'number' || !isFinite(b)) return '—'
    var units = [['TB', 1e12], ['GB', 1e9], ['MB', 1e6], ['KB', 1e3]]
    for (var i = 0; i < units.length; i++) {
      var u = units[i]
      if (b >= u[1]) {
        var x = b / u[1]
        return (x < 10 ? x.toFixed(1) : Math.round(x)) + ' ' + u[0]
      }
    }
    return Math.round(b) + ' B'
  }

  function gib(b) {
    if (typeof b !== 'number') return '—'
    var g = b / 1073741824
    return (Math.abs(g - Math.round(g)) < 0.05 ? Math.round(g) : g.toFixed(1)) + ' GiB'
  }

  function ago(t) {
    if (typeof t !== 'number') return 'not yet'
    var s = Math.max(0, Math.round(Date.now() / 1000 - t))
    if (s < 120) return s + ' s ago'
    if (s < 7200) return Math.round(s / 60) + ' min ago'
    return Math.round(s / 3600) + ' h ago'
  }

  var RANK = { ok: 0, warn: 1, refuse: 2, critical: 3 }
  var WORDS = { ok: 'OK', warn: 'Warn', refuse: 'Refusing', critical: 'Critical' }

  function worst(disks) {
    var w = 'ok'
    for (var i = 0; i < disks.length; i++) {
      var l = disks[i].level
      if (RANK[l] !== undefined && RANK[l] > RANK[w]) w = l
    }
    return w
  }

  function pct(x) {
    return Math.max(0, Math.min(100, Number(x) || 0))
  }

  function diskBlock(d, lines) {
    var box = el('div', 'bd-disk')
    var row = el('div', 'bd-row')
    row.appendChild(el('span', 'bd-mount', d.mount))
    row.appendChild(el('span', 'bd-chip bd-' + d.level, WORDS[d.level] || d.level))
    row.appendChild(el('span', 'bd-num', Math.round(d.usedPct) + '% · ' + size(d.freeBytes) + ' free'))
    box.appendChild(row)
    var bar = el('div', 'bd-bar')
    bar.setAttribute('role', 'meter')
    bar.setAttribute('aria-label', d.mount + ' used')
    bar.setAttribute('aria-valuemin', '0')
    bar.setAttribute('aria-valuemax', '100')
    bar.setAttribute('aria-valuenow', String(Math.round(d.usedPct)))
    var fill = el('span', 'bd-fill bd-' + d.level)
    fill.style.width = pct(d.usedPct) + '%'
    bar.appendChild(fill)
    var marks = [lines.warnPct, lines.refusePct, lines.criticalPct]
    for (var i = 0; i < marks.length; i++) {
      if (typeof marks[i] !== 'number') continue
      var t = el('span', 'bd-tick')
      t.style.left = 'calc(' + pct(marks[i]) + '% - 1px)'
      bar.appendChild(t)
    }
    box.appendChild(bar)
    box.appendChild(el('div', 'bd-lines', size(d.usedBytes) + ' used of ' + size(d.totalBytes) + (d.holds && d.holds.length ? ' · holds ' + d.holds.join(', ') : '')))
    return box
  }

  function list(title, rows, label, kind) {
    if (!rows || !rows.length) return null
    var frag = document.createDocumentFragment()
    frag.appendChild(el('h3', 'bd-section', title))
    var ul = el('ul', 'bd-list')
    var top = rows[0].diskBytes || 1
    for (var i = 0; i < rows.length; i++) {
      var r = rows[i]
      var li = el('li', 'bd-item')
      var name = el('span', 'bd-name', label(r))
      var k = kind(r)
      if (k) {
        name.appendChild(document.createTextNode(' '))
        name.appendChild(el('span', 'bd-kind', k))
      }
      li.appendChild(name)
      li.appendChild(el('span', 'bd-num', size(r.diskBytes)))
      var mini = el('div', 'bd-mini')
      var fill = el('span')
      fill.style.width = pct((r.diskBytes / top) * 100) + '%'
      mini.appendChild(fill)
      li.appendChild(mini)
      ul.appendChild(li)
    }
    frag.appendChild(ul)
    return frag
  }

  function draw(v, err) {
    clear(root)
    var title = (k2 && k2.config && typeof k2.config.title === 'string' && k2.config.title.trim()) || 'Box disk'
    var head = el('div', 'bd-head')
    head.appendChild(el('span', 'bd-title', title))
    if (v && v.host) head.appendChild(el('span', 'bd-host', v.host))
    root.appendChild(head)
    if (!v) {
      var why = err && err.code === 'verb_unavailable'
        ? 'This K2 doesn’t measure box usage yet (it arrives with 0.46.4).'
        : err ? 'K2 couldn’t read this computer’s disks: ' + (err.code || err.message || 'error') : 'Loading…'
      root.appendChild(el('p', 'bd-notice', why))
      return
    }
    var disks = Array.isArray(v.disks) ? v.disks : []
    var lines = (v.guard && v.guard.lines) || {}
    var mode = (v.guard && v.guard.mode) || 'off'
    var w = worst(disks)
    head.appendChild(el('span', 'bd-chip bd-' + w, mode === 'off' ? 'Guard off' : 'Guard ' + mode + ' · ' + (WORDS[w] || w)))
    if (!disks.length) root.appendChild(el('p', 'bd-notice', 'No disk could be read.'))
    for (var i = 0; i < disks.length; i++) root.appendChild(diskBlock(disks[i], lines))
    root.appendChild(el('div', 'bd-lines',
      'Lines: warn ' + lines.warnPct + '% or ' + gib(lines.warnFreeBytes) + ' free · refuse ' + lines.refusePct + '% or ' +
      gib(lines.reserveBytes) + ' left · critical ' + lines.criticalPct + '%'))
    var users = list('Biggest users', v.top, function (r) { return r.id }, function (r) {
      return r.kind === 'shared' ? 'server' : r.kind + (r.owner ? ' · ' + r.owner : '')
    })
    if (users) root.appendChild(users)
    var owners = list('Agents', v.owners, function (r) { return r.handle || r.address || 'an agent' }, function (r) { return r.address || '' })
    if (owners) root.appendChild(owners)
    var foot = el('div', 'bd-foot')
    if (typeof v.k2WillClearBytes === 'number' && v.k2WillClearBytes > 0) {
      foot.appendChild(el('span', null, 'K2 clears ' + size(v.k2WillClearBytes) + ' of its own old Seeds at the next sweep.'))
    }
    var mem = v.mem || {}
    if (typeof mem.totalBytes === 'number') {
      foot.appendChild(el('span', null, 'RAM ' + gib(mem.totalBytes) + (typeof mem.availableBytes === 'number' ? ' · ' + gib(mem.availableBytes) + ' available' : '')))
    }
    foot.appendChild(el('span', null, 'Folders measured ' + ago(v.scannedAt) + ' · read ' + ago(v.checkedAt)))
    root.appendChild(foot)
  }

  function load() {
    var p
    try {
      p = k2.app.resources()
    } catch (e) {
      p = Promise.reject(e)
    }
    return p.then(function (v) {
      last = v
      draw(v, null)
    }, function (err) {
      draw(last, last ? null : err)
    }).then(function () {
      if (!readied) {
        readied = true
        k2.ready()
      }
    })
  }

  draw(null, null)
  k2.connected.then(function () {
    load()
    setInterval(load, EVERY_MS)
  }).catch(function (err) {
    clear(root)
    root.appendChild(el('p', 'bd-notice', 'K2 didn’t connect this card: ' + ((err && err.message) || err)))
  })
})()
