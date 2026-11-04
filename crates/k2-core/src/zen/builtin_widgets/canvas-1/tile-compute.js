// The Compute load tile (k2:canvas@1): jobs on each of this server's
// compute nodes, node-wide, so a node with room stands out. It reads K2's
// `compute.nodes` (compute:nodes) every 30 s while the tile is on screen:
// the same facts as `k2 compute status` and `k2 compute queue <node>`.
//
// Per node: running jobs against its slots, queued jobs (every server's on
// a node several servers share), free CPU and memory, whose jobs they are,
// and why a node isn't taking jobs. Offline nodes fold into one line. The
// order (busiest first, or by name) is the person's private save.
// Read only: it writes and posts nothing.
(function () {
  'use strict'
  var k2 = window.k2
  var POLL_MS = 30000
  var STALE_SECS = 120

  function el(tag, cls, text) {
    var e = document.createElement(tag)
    if (cls) e.className = cls
    if (text != null) e.textContent = String(text)
    return e
  }
  function num(v) { return typeof v === 'number' && isFinite(v) ? v : null }
  function nowSecs() { return Math.floor(Date.now() / 1000) }
  function ago(secs) {
    if (secs == null) return ''
    if (secs < 60) return secs + ' s'
    if (secs < 3600) return Math.round(secs / 60) + ' min'
    if (secs < 86400) return Math.round(secs / 3600) + ' h'
    return Math.round(secs / 86400) + ' d'
  }
  function cpus(m) { return (Math.round(m / 100) / 10).toString() }
  function gb(b) { var g = b / 1073741824; return g >= 10 ? Math.round(g).toString() : (Math.round(g * 10) / 10).toString() }
  function plat(n) {
    var os = n.os === 'macos' ? 'mac' : (n.os || '?')
    var arch = n.arch === 'x86_64' ? 'x64' : (n.arch === 'arm64' || n.arch === 'aarch64' ? 'arm' : (n.arch || ''))
    return (os + ' ' + arch).trim()
  }
  function plural(n, one, many) { return n + ' ' + (n === 1 ? one : many) }
  function words(err) {
    var code = (err && err.code) || 'failed'
    return code.replace(/_/g, ' ') + (err && err.message ? ': ' + err.message : '')
  }

  // A node's facts the tile reasons on (never trust a field's presence).
  function facts(n) {
    var slots = num(n.slots) || 0
    var running = num(n.running) || 0
    var queued = num(n.queued) || 0
    var cpu = n.cpu || {}
    var mem = n.mem || {}
    var cpuF = num(cpu.freeMillis), cpuT = num(cpu.totalMillis)
    var memF = num(mem.freeBytes), memT = num(mem.totalBytes)
    var online = n.online === true
    var active = online && n.state === 'active'
    var held = active && typeof n.why === 'string' && n.why !== ''
    var room = active && !held && queued === 0 && cpuF != null && cpuF > 0 && running < Math.max(slots, 1)
    var full = slots > 0 && running >= slots
    return {
      slots: slots, running: running, queued: queued, cpuF: cpuF, cpuT: cpuT, memF: memF, memT: memT,
      online: online, active: active, held: held, room: room, full: full,
      ratio: slots ? running / slots : (running ? 1 : 0),
      cpuUsed: cpuT ? (cpuT - (cpuF || 0)) / cpuT : 0,
    }
  }

  function byName(a, b) {
    return String(a.n.name).localeCompare(String(b.n.name), undefined, { numeric: true })
  }

  window.K2Canvas.tile({
    id: 'compute',
    name: 'Compute load',
    description: 'Jobs on each compute node: running against its slots, queued, free CPU and memory, and whose jobs they are.',
    size: 'full',
    needs: ['compute.nodes'],
    mount: function (ctx) {
      var snap = null
      var gotAt = null
      var problem = null
      var sort = 'busy'
      var showOff = false
      var timer = 0
      var ticker = 0
      var stopped = false

      var root = el('div', 'cl')
      var bar = el('div', 'cl-head')
      var seg = el('div', 'cl-seg')
      seg.setAttribute('role', 'group')
      seg.setAttribute('aria-label', 'Order')
      var busyBtn = el('button', 'cl-btn', 'Busiest')
      busyBtn.type = 'button'
      busyBtn.title = 'Busiest nodes first (running ÷ slots, then queued, then CPU in use)'
      var nameBtn = el('button', 'cl-btn', 'A–Z')
      nameBtn.type = 'button'
      nameBtn.title = 'By node name'
      seg.appendChild(busyBtn)
      seg.appendChild(nameBtn)
      var meta = el('span', 'cl-meta')
      meta.setAttribute('aria-live', 'polite')
      bar.appendChild(meta)
      bar.appendChild(seg)
      root.appendChild(bar)
      var online = el('div', 'cl-grid')
      online.setAttribute('role', 'list')
      online.setAttribute('aria-label', 'Online compute nodes')
      root.appendChild(online)
      var offWrap = el('div', 'cl-off')
      var offToggle = el('button', 'cl-off-head')
      offToggle.type = 'button'
      var offline = el('div', 'cl-grid cl-grid-off')
      offline.setAttribute('role', 'list')
      offline.setAttribute('aria-label', 'Offline compute nodes')
      offWrap.appendChild(offToggle)
      offWrap.appendChild(offline)
      root.appendChild(offWrap)
      var legend = el('div', 'cl-legend')
      legend.setAttribute('aria-hidden', 'true')
      ;[['cl-sw-run', 'running'], ['cl-sw-q', 'queued'], ['cl-sw-free', 'free slot']].forEach(function (p) {
        var s = el('span')
        s.appendChild(el('i', 'cl-sw ' + p[0]))
        s.appendChild(document.createTextNode(p[1]))
        legend.appendChild(s)
      })
      legend.appendChild(el('span', 'cl-legend-room', 'room = taking jobs, CPU free, nothing queued'))
      root.appendChild(legend)
      ctx.body.appendChild(root)

      function order(rows) {
        return rows.slice().sort(function (a, b) {
          if (sort === 'name') return byName(a, b)
          return (b.f.ratio - a.f.ratio) || (b.f.queued - a.f.queued) || (b.f.cpuUsed - a.f.cpuUsed) || byName(a, b)
        })
      }

      function resLine(row, label, free, total, fmt, unit) {
        row.appendChild(el('span', null, label))
        var track = el('span', 'cl-res-track')
        var fill = el('span', 'cl-res-fill')
        var used = total ? Math.max(0, Math.min(1, (total - free) / total)) : 0
        fill.style.width = Math.round(used * 100) + '%'
        if (used >= 0.9) fill.classList.add('cl-hot')
        track.appendChild(fill)
        row.appendChild(track)
        row.appendChild(el('span', 'cl-res-val', fmt(free) + '/' + fmt(total) + unit + ' free'))
      }

      function jobsTitle(n, f) {
        var lines = [n.name + ' · ' + plat(n) + (n.version ? ' · K2 node ' + n.version : '')]
        lines.push('running ' + f.running + ' of ' + f.slots + ' slots · queued ' + f.queued + (n.queueFrom === 'server' && n.shared ? ' (this server’s only)' : ''))
        if (n.shared && num(n.servers)) lines.push('shared by ' + plural(n.servers, 'server', 'servers'))
        ;(Array.isArray(n.jobs) ? n.jobs : []).forEach(function (j) {
          var who = j.workspace || j.server || 'job'
          var t = num(j.elapsedSecs) != null ? ' ' + ago(j.elapsedSecs) : ''
          var why = j.blockedBy ? ' (' + String(j.blockedBy).replace(/_/g, ' ') + ')' : ''
          lines.push('  ' + (j.state === 'running' ? '▶ ' : '… ') + who + t + why)
        })
        if (Array.isArray(n.load) && num(n.load[0]) != null) lines.push('load ' + n.load.map(function (x) { return Math.round(x * 100) / 100 }).join(' '))
        if (n.why) lines.push(String(n.why))
        if (n.queueError) lines.push('queue unread: ' + n.queueError)
        return lines.join('\n')
      }

      function card(n, f, old) {
        var t = el('div', 'cl-node')
        t.setAttribute('role', 'listitem')
        t.tabIndex = 0
        if (!f.online) t.classList.add('cl-down')
        else if (!f.active || f.held) t.classList.add('cl-blocked')
        else if (f.room) t.classList.add('cl-room')
        else if (f.full) t.classList.add('cl-full')
        else if (f.running) t.classList.add('cl-busy')
        if (old) t.classList.add('cl-old')
        t.title = jobsTitle(n, f)

        var top = el('div', 'cl-top')
        var dot = el('span', 'cl-dot')
        if (f.active) dot.classList.add('cl-on')
        else if (f.online) dot.classList.add('cl-pause')
        top.appendChild(dot)
        top.appendChild(el('span', 'cl-name', n.name))
        top.appendChild(el('span', 'cl-plat', plat(n)))
        var count = el('span', 'cl-count', f.running)
        count.appendChild(el('small', null, '/' + f.slots))
        top.appendChild(count)
        t.appendChild(top)

        // One segment per slot (filled = running), then the queue.
        var b = el('div', 'cl-bar')
        b.setAttribute('aria-label', f.running + ' of ' + f.slots + ' slots running, ' + f.queued + ' queued')
        for (var i = 0; i < Math.max(f.slots, 1); i++) {
          var s = el('span', 'cl-seg-slot')
          if (i < f.running) s.classList.add('cl-on')
          b.appendChild(s)
        }
        var shownQ = Math.min(f.queued, 4)
        for (var q = 0; q < shownQ; q++) b.appendChild(el('span', 'cl-seg-q'))
        if (f.queued > shownQ) b.appendChild(el('span', 'cl-seg-more', '+' + (f.queued - shownQ)))
        t.appendChild(b)

        if (f.cpuT || f.memT) {
          var res = el('div', 'cl-res')
          if (f.cpuT) resLine(res, 'cpu', f.cpuF || 0, f.cpuT, cpus, '')
          if (f.memT) resLine(res, 'mem', f.memF || 0, f.memT, gb, ' GB')
          t.appendChild(res)
        }

        var tags = el('div', 'cl-tags')
        if (!f.online) {
          tags.appendChild(el('span', 'cl-tag', num(n.lastSeenAt) ? 'seen ' + ago(Math.max(0, nowSecs() - n.lastSeenAt)) + ' ago' : 'offline'))
        } else if (!f.active) {
          tags.appendChild(el('span', 'cl-tag cl-t-state', n.why ? n.why : String(n.state)))
        } else if (f.held) {
          tags.appendChild(el('span', 'cl-tag cl-t-state', n.why))
        }
        if (f.room) tags.appendChild(el('span', 'cl-tag cl-t-room', 'room: ' + plural(Math.max(f.slots - f.running, 1), 'slot', 'slots')))
        if (n.shared && n.queueFrom === 'server' && f.online) {
          var only = el('span', 'cl-tag', 'this server’s jobs only')
          only.title = n.queueError ? 'The node didn’t send its queue: ' + n.queueError : 'Counts are this server’s jobs'
          tags.appendChild(only)
        }
        var who = (Array.isArray(n.who) ? n.who : []).map(function (w) {
          var bits = []
          if (w.running) bits.push(w.running + ' run')
          if (w.queued) bits.push(w.queued + ' queued')
          return w.label + ' ' + bits.join(', ')
        })
        if (who.length) tags.appendChild(el('span', 'cl-who', who.join(' · ')))
        if (tags.childNodes.length) t.appendChild(tags)
        return t
      }

      function draw() {
        var age = gotAt == null ? null : Math.max(0, nowSecs() - gotAt)
        var old = age != null && age > STALE_SECS
        ctx.setMeta(age == null ? '' : (age < 5 ? 'now' : ago(age) + ' ago'))
        var note = problem
        if (!note && old) note = 'Last read ' + ago(age) + ' ago: K2 hasn’t answered since. Numbers are greyed until it does.'
        ctx.note(note)

        online.textContent = ''
        offline.textContent = ''
        meta.textContent = ''
        if (!snap) {
          if (!problem) online.appendChild(el('p', 'cl-empty', 'Reading the compute nodes…'))
          offWrap.hidden = true
          return
        }
        var rows = (Array.isArray(snap.nodes) ? snap.nodes : []).filter(function (n) { return n && n.name }).map(function (n) {
          return { n: n, f: facts(n) }
        })
        var on = order(rows.filter(function (r) { return r.f.online }))
        var off = rows.filter(function (r) { return !r.f.online }).sort(byName)

        var run = 0, slots = 0, queued = 0, room = 0
        on.forEach(function (r) {
          run += r.f.running; slots += r.f.slots; queued += r.f.queued
          if (r.f.room) room++
        })
        var part = function (b, text, cls) {
          meta.appendChild(el('b', cls || null, b))
          meta.appendChild(document.createTextNode(text))
        }
        part(run + '/' + slots, ' running · ')
        part(String(queued), ' queued · ')
        part(String(room), ' with room', 'cl-room-n')
        meta.appendChild(document.createTextNode(' · ' + on.length + ' online'))

        if (!rows.length) online.appendChild(el('p', 'cl-empty', 'No compute nodes on this server yet (k2 study compute says how to add one).'))
        else if (!on.length) online.appendChild(el('p', 'cl-empty', 'No node is online.'))
        on.forEach(function (r) { online.appendChild(card(r.n, r.f, old)) })

        offWrap.hidden = !off.length
        offToggle.textContent = (showOff ? '▾ ' : '▸ ') + 'offline (' + off.length + '): ' + off.map(function (r) { return r.n.name }).join(', ')
        offToggle.setAttribute('aria-expanded', showOff ? 'true' : 'false')
        offline.hidden = !showOff
        if (showOff) off.forEach(function (r) { offline.appendChild(card(r.n, r.f, old)) })
      }

      function poll() {
        clearTimeout(timer)
        if (stopped || !ctx.visible()) return
        Promise.resolve().then(function () { return k2.compute.nodes() }).then(function (v) {
          snap = v && typeof v === 'object' ? v : null
          gotAt = snap && num(snap.at) != null ? snap.at : nowSecs()
          problem = null
        }, function (err) {
          problem = 'K2 didn’t read the nodes (' + words(err) + ').'
        }).then(function () {
          if (stopped) return
          draw()
          timer = setTimeout(poll, POLL_MS)
        })
      }

      function setSort(s, keep) {
        sort = s
        busyBtn.setAttribute('aria-pressed', s === 'busy' ? 'true' : 'false')
        nameBtn.setAttribute('aria-pressed', s === 'name' ? 'true' : 'false')
        draw()
        if (keep) ctx.save.set('sort', s).catch(function () { /* a convenience only */ })
      }
      busyBtn.addEventListener('click', function () { setSort('busy', true) })
      nameBtn.addEventListener('click', function () { setSort('name', true) })
      offToggle.addEventListener('click', function () { showOff = !showOff; draw() })
      ctx.onVisible(function (v) { if (v) poll(); else clearTimeout(timer) })

      setSort('busy', false)
      ctx.save.get('sort').then(function (s) { if (s === 'busy' || s === 'name') setSort(s, false) }, function () { /* the default order */ })
      poll()
      ticker = setInterval(function () { if (!stopped) draw() }, 15000)

      return {
        stop: function () {
          stopped = true
          clearTimeout(timer)
          clearInterval(ticker)
        },
      }
    },
  })
})()
