// K2's card kit, second version (0.46.1), for the `k2:*@2` data-source
// cards. Every @2 card includes this file byte for byte (`include_str!`), so
// each card version stays one flat, immutable file set; `sources-kit/` stays
// as @1 shipped it.
//
// What @2 adds to the @1 kit:
//   - several sources per card: `config.source` plus `config.sources` (a
//     comma list), merged: items with the same key across sources become one
//     item (fields filled in source order, the worst health wins), counts are
//     recounted, the card's state is the worst live state, and each source
//     that needs a word (stale, removed, unreachable, not saved) gets its own
//     line;
//   - a soft reading waiting to go hard: "soft crit 2/3" from `soft.level`
//     (the 0.46.1 card view), "retrying 2/3" from a pre-0.46.1 server;
//   - named lists: items carrying `list` (keys `<list>/<key>`) stay out of
//     the main grid; `mainItems(view)` / `listItems(view, name)` pick them;
//   - a refused or ended subscription subscribes again by itself, after
//     5 s, doubling to at most 60 s, and back to 5 s on the next good view;
//     the card keeps drawing the last state meanwhile.
// Unchanged from @1: the header (state chip, "updated N ago" from the
// server's clock, recovered, muted and TEST badges, the link's host), the
// notices, the renderers and "Ask an agent". Never blank after the first
// load. Agent and source text only ever goes through textContent; no inline
// handlers; no network or storage.
(function () {
  'use strict'

  var TICK_MS = 15000
  var HOUR = 3600
  var RETRY_FIRST_MS = 5000
  var RETRY_MAX_MS = 60000
  var RANK = { ok: 0, unknown: 1, warn: 2, crit: 3 }

  function el(tag, cls, text) {
    var e = document.createElement(tag)
    if (cls) e.className = cls
    if (text !== undefined && text !== null) e.textContent = String(text)
    return e
  }

  function clear(node) {
    while (node.firstChild) node.removeChild(node.firstChild)
  }

  /** "just now", "40 s ago", "3 min ago", "2 h ago", "4 d ago". */
  function ago(secs) {
    if (secs === null || secs === undefined || !isFinite(secs)) return ''
    var s = Math.max(0, Math.round(secs))
    if (s < 10) return 'just now'
    if (s < 60) return s + ' s ago'
    if (s < HOUR) return Math.floor(s / 60) + ' min ago'
    if (s < 86400) return Math.floor(s / HOUR) + ' h ago'
    return Math.floor(s / 86400) + ' d ago'
  }

  /** Wall time HH:MM on this computer for seconds since the epoch. */
  function clock(t) {
    var d = new Date(t * 1000)
    var p = function (n) { return (n < 10 ? '0' : '') + n }
    return p(d.getHours()) + ':' + p(d.getMinutes())
  }

  function hostOf(address) {
    var i = String(address || '').indexOf('::')
    return i < 0 ? null : address.slice(i + 2)
  }

  function nameOf(address) {
    var s = String(address || '')
    var i = s.indexOf('::')
    return i < 0 ? s : s.slice(0, i)
  }

  function linkHost(link) {
    var m = /^https?:\/\/([^/?#:]+)/i.exec(String(link || ''))
    return m ? m[1] : null
  }

  var WORDS = { ok: 'OK', warn: 'Warning', crit: 'Down', unknown: 'Unknown', stale: 'Stale', pending: 'Waiting', paused: 'Paused' }

  /** The CSS state for a view or item: its display, greyed when not live. */
  function stateOf(display) {
    return WORDS[display] ? display : 'unknown'
  }

  /** The worse of two healths (unknown ranks between ok and warn). */
  function worse(a, b) {
    if (RANK[a] === undefined) return b
    if (RANK[b] === undefined) return a
    return RANK[b] > RANK[a] ? b : a
  }

  /** "soft crit 2/3": a reading waiting to go hard. A pre-0.46.1 server
   *  sends no level: "retrying 2/3". '' when nothing waits. */
  function softHint(soft) {
    if (!soft || typeof soft !== 'object' || !(soft.count > 0)) return ''
    if (typeof soft.level === 'string' && soft.level) return 'soft ' + soft.level + ' ' + soft.count + '/' + soft.of
    return soft.count < soft.of ? 'retrying ' + soft.count + '/' + soft.of : ''
  }

  /** Seconds now on the source's server: its serverNow plus local elapsed. */
  function serverNow(view) {
    if (!view) return null
    var at = view.__receivedAt || Date.now()
    return view.serverNow + (Date.now() - at) / 1000
  }

  /** The notice for a view (or null): the PRD §12 failure states. */
  function notice(view, address) {
    var host = hostOf(address)
    var where = host || 'this computer'
    if (!view) return null
    if (view.reach === 'server_not_saved') return { kind: 'notice', text: 'Add ' + host + ' to your servers (Settings → K2 Server → Connected Servers).' }
    if (view.reach === 'cap_not_granted') return { kind: 'notice', text: 'Your login on ' + where + ' can’t read this source.' }
    if (view.reach === 'removed' || view.status === 'removed') return { kind: 'gone', text: 'Removed on ' + where + '.' }
    if (view.reach === 'unreachable') return { kind: 'gone', text: (host || 'K2') + ' unreachable: showing the last check.' }
    if (view.reach === 'not_bound') return { kind: 'notice', text: 'Not bound to this card yet; trying again.' }
    if (view.reach === 'failed') return { kind: 'notice', text: view.__error || 'K2 couldn’t read this source.' }
    var gap = view.gap
    if (gap && (view.updatedAt === null || view.updatedAt < gap.to)) {
      var span = ' (' + clock(gap.from) + '–' + clock(gap.to) + ')'
      if (gap.kind === 'host_slept') return { kind: 'gone', text: (host ? 'Paused while ' + host + ' slept' : 'Paused while this computer slept') + span + '.' }
      return { kind: 'gone', text: 'K2 restarted' + (host ? ' on ' + host : '') + span + '.' }
    }
    if (view.phase === 'pending') return { kind: 'notice', text: 'Waiting for the first check.' }
    if (view.phase === 'paused') return { kind: 'gone', text: 'Paused' + (view.reasonText ? ': ' + view.reasonText : '.') }
    if (view.phase === 'stale') return { kind: 'gone', text: 'Stale: ' + (view.reasonText || 'no good check lately.') }
    return null
  }

  /** Is the view greyed (the last good value, not a live one)? */
  function greyed(view) {
    var n = notice(view, view && view.address)
    return !!(n && n.kind === 'gone')
  }

  /** A real card view (not a refusal with no last view). */
  function isView(v) {
    return !!(v && typeof v === 'object' && typeof v.serverNow === 'number')
  }

  function itemsOf(view) {
    return view && Array.isArray(view.items) ? view.items.filter(function (it) { return it && typeof it === 'object' && typeof it.key === 'string' }) : []
  }

  /** Items of the main list (no `list`). */
  function mainItems(view) {
    return itemsOf(view).filter(function (it) { return !it.list })
  }

  /** Items of one named list (`extract.lists`), keys without the prefix. */
  function listItems(view, name) {
    return itemsOf(view).filter(function (it) { return it.list === name })
  }

  // ── merging several sources ────────────────────────────────────────

  function countsOf(items) {
    var c = { ok: 0, warn: 0, crit: 0, unknown: 0 }
    for (var i = 0; i < items.length; i++) {
      var h = items[i].health
      c[RANK[h] === undefined ? 'unknown' : h] += 1
    }
    return c
  }

  /**
   * One view from several `[{address, view}]` (in config order). One
   * source: its view as is. Several: items merged by key (fields filled in
   * source order, worst health, its reason and soft), list items kept apart
   * per source, fields filled in source order, counts recounted, health and
   * display the worst live ones, updatedAt the oldest live one.
   */
  function merge(sources) {
    var live = sources.filter(function (s) { return isView(s.view) })
    if (sources.length === 1) return live.length ? live[0].view : null
    if (!live.length) return null
    var base = live[0].view
    var byKey = {}
    var order = []
    var fields = {}
    var health = null
    var display = null
    var updatedAt = null
    var soft = null
    var allGreyed = true
    for (var i = 0; i < live.length; i++) {
      var v = live[i].view
      var g = greyed(v)
      if (!g) {
        allGreyed = false
        health = health === null ? v.health : worse(health, v.health)
        display = display === null ? v.display : (RANK[v.display] !== undefined && RANK[display] !== undefined ? worse(display, v.display) : display)
        if (typeof v.updatedAt === 'number') updatedAt = updatedAt === null ? v.updatedAt : Math.min(updatedAt, v.updatedAt)
      }
      if (!soft && v.soft && v.soft.count > 0) soft = v.soft
      var f = v.fields && typeof v.fields === 'object' ? v.fields : {}
      for (var k in f) if (Object.prototype.hasOwnProperty.call(f, k) && (fields[k] === undefined || fields[k] === null)) fields[k] = f[k]
      var its = itemsOf(v)
      for (var j = 0; j < its.length; j++) {
        var it = its[j]
        var key = it.list ? live[i].address + '\u0000' + it.key : it.key
        var have = byKey[key]
        if (!have) {
          byKey[key] = {
            key: it.key,
            list: it.list,
            health: g ? 'unknown' : it.health,
            reason: it.reason,
            since: it.since,
            soft: it.soft,
            fields: Object.assign({}, it.fields || {}),
            source: live[i].address,
            __grey: g,
          }
          order.push(key)
          continue
        }
        var itf = it.fields || {}
        for (var n in itf) if (Object.prototype.hasOwnProperty.call(itf, n) && (have.fields[n] === undefined || have.fields[n] === null)) have.fields[n] = itf[n]
        // A greyed source fills fields but never colours a box a live source
        // rates; a live source replaces a greyed-only reading.
        if (g) continue
        if (have.__grey) {
          have.__grey = false
          have.health = it.health
          have.reason = it.reason
          have.since = it.since
          have.source = live[i].address
          continue
        }
        var h = it.health
        if (worse(have.health, h) !== have.health) {
          have.health = h
          have.reason = it.reason
          have.since = it.since
        }
        if (!(have.soft && have.soft.count > 0) && it.soft && it.soft.count > 0) have.soft = it.soft
      }
    }
    var items = order.map(function (k) {
      var it = byKey[k]
      delete it.__grey
      return it
    })
    var out = Object.assign({}, base)
    out.items = items
    out.fields = fields
    out.counts = countsOf(items)
    if (!allGreyed) {
      out.health = health
      out.display = display
      out.phase = 'live'
      out.gap = null
      out.reach = 'ok'
      if (updatedAt !== null) out.updatedAt = updatedAt
    }
    out.soft = soft || { count: 0, of: base.soft ? base.soft.of : 0 }
    var mutedSrc = live.filter(function (s) { return s.view.muted })[0]
    out.muted = mutedSrc ? mutedSrc.view.muted : null
    out.__sources = live.length
    return out
  }

  // ── the header and notice ─────────────────────────────────────────

  function header(root, view, opts) {
    opts = opts || {}
    var head = el('header', 'k2c-head')
    var title = el('h1', 'k2c-title', opts.title || (view && view.label) || (opts.address || 'Data source'))
    head.appendChild(title)
    if (view) {
      var disp = greyed(view) ? 'stale' : stateOf(view.display)
      var chip = el('span', 'k2c-chip k2c-' + disp, WORDS[disp])
      chip.setAttribute('data-k2c-state', disp)
      head.appendChild(chip)
    }
    root.appendChild(head)
    var meta = el('div', 'k2c-meta')
    if (view) {
      var now = serverNow(view)
      if (view.updatedAt !== null && view.updatedAt !== undefined) {
        meta.appendChild(el('span', 'k2c-age', 'updated ' + ago(now - view.updatedAt)))
      } else {
        meta.appendChild(el('span', 'k2c-age', 'no check yet'))
      }
      var sh = softHint(view.soft)
      if (sh) {
        var s = el('span', 'k2c-soft', sh)
        if (view.soft.level) s.setAttribute('data-k2c-level', view.soft.level)
        meta.appendChild(s)
      }
      if (view.recoveredAt && now - view.recoveredAt < HOUR && view.display === 'ok') {
        meta.appendChild(el('span', 'k2c-recovered', 'recovered ' + ago(now - view.recoveredAt)))
      }
      if (view.muted) {
        meta.appendChild(el('span', 'k2c-badge k2c-muted', 'Muted by ' + view.muted.by + ' until ' + clock(view.muted.until)))
      }
      if (view.simulated) {
        var t = el('span', 'k2c-badge k2c-test', 'TEST: ' + view.simulated.state + ' until ' + clock(view.simulated.until))
        if (view.simulated.by) t.title = 'Set by ' + view.simulated.by
        meta.appendChild(t)
      }
      if (view.__sources > 1) meta.appendChild(el('span', 'k2c-badge', view.__sources + ' sources'))
      var lh = linkHost(view.link)
      if (lh) {
        var l = el('span', 'k2c-link', '↗ ' + lh)
        l.title = 'Open it from the card’s ⋯ menu'
        meta.appendChild(l)
      }
    }
    root.appendChild(meta)
    if (opts.sources && opts.sources.length > 1) {
      // Several sources: one line per source that needs a word.
      var box = null
      for (var i = 0; i < opts.sources.length; i++) {
        var src = opts.sources[i]
        var sn = src.view ? notice(src.view, src.address) : { kind: 'notice', text: 'Loading…' }
        if (!sn) continue
        if (!box) { box = el('div', 'k2c-sources'); box.setAttribute('role', 'status') }
        var line = el('p', 'k2c-source-line k2c-notice-' + sn.kind, nameOf(src.address) + ': ' + sn.text)
        line.setAttribute('data-k2c-source', src.address)
        box.appendChild(line)
      }
      if (box) root.appendChild(box)
      return
    }
    var n = notice(view, opts.address)
    if (n) {
      var nb = el('p', 'k2c-notice k2c-notice-' + n.kind, n.text)
      nb.setAttribute('role', 'status')
      root.appendChild(nb)
    }
  }

  // ── renderers ─────────────────────────────────────────────────────

  /** Tiles, one per item. `opts.labelField`, `opts.detailFields`,
   *  `opts.marks(item) → [text]`, `opts.ask`. */
  function statusGrid(root, items, opts) {
    opts = opts || {}
    var grid = el('div', 'k2c-grid')
    grid.setAttribute('role', 'list')
    for (var i = 0; i < items.length; i++) {
      var it = items[i]
      var st = opts.stale ? 'stale' : stateOf(it.health)
      var tile = el('div', 'k2c-tile k2c-' + st)
      tile.setAttribute('role', 'listitem')
      tile.setAttribute('data-k2c-item', it.key)
      tile.setAttribute('data-k2c-state', st)
      var label = opts.labelField && it.fields && it.fields[opts.labelField] !== undefined && it.fields[opts.labelField] !== null
        ? it.fields[opts.labelField] : it.key
      tile.appendChild(el('div', 'k2c-tile-name', label))
      var detail = []
      var fields = opts.detailFields || []
      for (var f = 0; f < fields.length; f++) {
        var v = it.fields ? it.fields[fields[f]] : undefined
        if (v !== undefined && v !== null && v !== '') detail.push(String(v))
      }
      if (opts.marks) {
        var mk = opts.marks(it) || []
        for (var q = 0; q < mk.length; q++) if (mk[q]) detail.push(String(mk[q]))
      }
      if (it.reason) detail.push(String(it.reason).replace(/_/g, ' '))
      var sh = softHint(it.soft)
      if (sh) detail.push(sh)
      if (detail.length) tile.appendChild(el('div', 'k2c-tile-detail', detail.join(' · ')))
      if (opts.ask && (st === 'crit' || st === 'stale' || st === 'unknown')) {
        var b = el('button', 'k2c-ask', 'Ask an agent')
        b.type = 'button'
        b.setAttribute('data-k2c-ask', it.key)
        b.addEventListener('click', (function (item) {
          return function () { opts.ask(item.key, item.source) }
        })(it))
        tile.appendChild(b)
      }
      grid.appendChild(tile)
    }
    if (!items.length) grid.appendChild(el('p', 'k2c-empty', opts.empty || 'Nothing to show yet.'))
    root.appendChild(grid)
    return grid
  }

  /** One line that says what's wrong, or that all is well. */
  function alertBanner(root, view, opts) {
    opts = opts || {}
    var c = view.counts || { ok: 0, warn: 0, crit: 0, unknown: 0 }
    var parts = []
    if (c.crit) parts.push(c.crit + ' ' + (opts.critWord || 'down'))
    if (c.warn) parts.push(c.warn + ' ' + (opts.warnWord || 'warning'))
    if (c.unknown) parts.push(c.unknown + ' unknown')
    var st = greyed(view) ? 'stale' : (c.crit ? 'crit' : c.warn ? 'warn' : c.unknown && !c.ok ? 'unknown' : stateOf(view.display))
    var text = parts.length ? parts.join(' · ') : (opts.okText || 'All ' + (c.ok || '') + ' OK').replace('All  OK', 'All OK')
    var b = el('div', 'k2c-banner k2c-' + st, text)
    b.setAttribute('data-k2c-banner', st)
    root.appendChild(b)
    return b
  }

  /** A sparkline path for points [{t, v}] (v null breaks the line). */
  function sparkPath(points, w, h) {
    var vs = []
    for (var i = 0; i < points.length; i++) if (typeof points[i].v === 'number') vs.push(points[i].v)
    if (vs.length < 2) return ''
    var lo = Math.min.apply(null, vs), hi = Math.max.apply(null, vs)
    var t0 = points[0].t, t1 = points[points.length - 1].t
    var span = t1 - t0 || 1, rng = hi - lo || 1
    var d = '', pen = false
    for (var j = 0; j < points.length; j++) {
      var p = points[j]
      if (typeof p.v !== 'number') { pen = false; continue }
      var x = ((p.t - t0) / span) * w
      var y = h - ((p.v - lo) / rng) * (h - 2) - 1
      d += (pen ? 'L' : 'M') + x.toFixed(1) + ' ' + y.toFixed(1) + ' '
      pen = true
    }
    return d.trim()
  }

  /** A big number with a label and an optional sparkline. */
  function numberTrend(root, o) {
    var box = el('div', 'k2c-number' + (o.state ? ' k2c-' + o.state : ''))
    box.appendChild(el('div', 'k2c-number-value', o.value === null || o.value === undefined ? '–' : o.value))
    if (o.label) box.appendChild(el('div', 'k2c-number-label', o.label))
    if (Array.isArray(o.points) && o.points.length > 1) {
      var NS = 'http://www.w3.org/2000/svg'
      var svg = document.createElementNS(NS, 'svg')
      svg.setAttribute('class', 'k2c-spark')
      svg.setAttribute('viewBox', '0 0 120 28')
      svg.setAttribute('preserveAspectRatio', 'none')
      svg.setAttribute('aria-hidden', 'true')
      var path = document.createElementNS(NS, 'path')
      path.setAttribute('d', sparkPath(o.points, 120, 28))
      svg.appendChild(path)
      box.appendChild(svg)
    }
    root.appendChild(box)
    return box
  }

  /** Rows [{key, state, primary, secondary, source?}]. */
  function list(root, rows, opts) {
    opts = opts || {}
    var ul = el('ul', 'k2c-list')
    for (var i = 0; i < rows.length; i++) {
      var r = rows[i]
      var st = opts.stale ? 'stale' : stateOf(r.state)
      var li = el('li', 'k2c-row k2c-' + st)
      li.setAttribute('data-k2c-item', r.key)
      li.setAttribute('data-k2c-state', st)
      li.appendChild(el('span', 'k2c-dot'))
      li.appendChild(el('span', 'k2c-row-main', r.primary))
      if (r.secondary) li.appendChild(el('span', 'k2c-row-side', r.secondary))
      if (opts.ask && (st === 'crit' || st === 'stale' || st === 'unknown')) {
        var b = el('button', 'k2c-ask', 'Ask')
        b.type = 'button'
        b.setAttribute('data-k2c-ask', r.key)
        b.addEventListener('click', (function (row) {
          return function () { opts.ask(row.key, row.source) }
        })(r))
        li.appendChild(b)
      }
      ul.appendChild(li)
    }
    if (!rows.length) ul.appendChild(el('li', 'k2c-empty', opts.empty || 'Nothing yet.'))
    root.appendChild(ul)
    return ul
  }

  function section(root, text) {
    var h = el('h2', 'k2c-section', text)
    root.appendChild(h)
    return h
  }

  // ── wiring ────────────────────────────────────────────────────────

  /** A refusal drawn as a state: a view-shaped object with a reach. */
  function refusalView(err, address) {
    var code = err && err.code
    if (err && isView(err.last)) {
      var last = err.last
      last.__receivedAt = Date.now()
      return last
    }
    var reach = code === 'server_not_saved' || code === 'cap_not_granted' || code === 'removed' || code === 'unreachable' || code === 'not_bound' ? code : null
    if (reach) return { address: address, reach: reach }
    return { address: address, reach: 'failed', __error: (err && err.message) || 'K2 couldn’t read this source.' }
  }

  /** `config.source` then each of `config.sources` (comma list), once each. */
  function addressesOf(cfg) {
    var out = []
    var add = function (s) {
      var a = String(s).trim()
      if (a && out.indexOf(a) < 0) out.push(a)
    }
    if (typeof cfg.source === 'string') cfg.source.split(',').forEach(add)
    if (typeof cfg.sources === 'string') cfg.sources.split(',').forEach(add)
    if (Array.isArray(cfg.sources)) cfg.sources.forEach(function (s) { if (typeof s === 'string') add(s) })
    return out
  }

  /**
   * mount({render(main, ctx), extra?: ['latest'], title?}): subscribe to
   * every address of config.source + config.sources (and the extra config
   * keys), call render on every change and every 15 s (ages), and tell K2
   * ready after the first draw. A refused or ended subscription subscribes
   * again by itself (5 s, doubling to 60 s).
   * ctx: {view (merged), views: [{address, view}], extra: {latest: view},
   *       address (the first), addresses, config, stale, ask(itemKey,
   *       address?), redraw()}.
   */
  function mount(o) {
    var root = document.getElementById('card')
    if (!root) throw new Error('card: missing #card')
    var state = { views: {}, extra: {}, addresses: [], readied: false, retry: {}, timers: {}, offs: {} }
    var sourcesNow = function () {
      return state.addresses.map(function (a) { return { address: a, view: state.views[a] || null } })
    }
    var draw = function () {
      clear(root)
      var first = state.addresses[0] || null
      var srcs = sourcesNow()
      var merged = merge(srcs)
      var firstView = first ? state.views[first] : null
      var ctx = {
        view: merged,
        views: srcs,
        extra: state.extra,
        address: first,
        addresses: state.addresses.slice(),
        config: k2.config,
        stale: !!(merged && greyed(merged)),
        ask: function (key, address) {
          var a = typeof address === 'string' && address ? address : first
          k2.sources.ask(a, key === undefined ? null : key).catch(function (e) { console.warn('card: ask failed', e) })
        },
        redraw: function () { draw() },
      }
      var title = (k2.config && k2.config.title) || o.title
      if (!first) {
        root.appendChild(el('p', 'k2c-notice', 'This card needs a source: set config.source in the Garden file (name, or name::host).'))
      } else if (!merged && srcs.length === 1 && firstView && firstView.__error) {
        header(root, null, { title: o.title || first, address: first })
        root.appendChild(el('p', 'k2c-notice', firstView.__error))
      } else if (!merged && srcs.every(function (s) { return !s.view })) {
        header(root, null, { title: o.title, address: first })
        root.appendChild(el('p', 'k2c-notice', 'Loading…'))
      } else if (!merged) {
        // Refusals with no last view: the notices alone, never a blank card.
        header(root, null, { title: (k2.config && k2.config.title) || first, address: first, sources: srcs })
        if (srcs.length === 1) {
          var n = notice(firstView, first)
          root.appendChild(el('p', 'k2c-notice', n ? n.text : 'K2 can’t read this source.'))
        }
      } else {
        var main = el('main', 'k2c-body' + (ctx.stale ? ' k2c-greyed' : ''))
        header(root, merged, { title: title, address: first, sources: srcs })
        root.appendChild(main)
        o.render(main, ctx)
      }
      if (!state.readied) {
        state.readied = true
        k2.ready()
      }
    }
    var subscribe = function (address, key) {
      var slot = key || address
      var on = function (v) {
        if (!v || typeof v !== 'object') return
        v.__receivedAt = Date.now()
        state.retry[slot] = RETRY_FIRST_MS
        if (key) state.extra[key] = v
        else state.views[address] = v
        draw()
      }
      var fail = function (err) {
        var v = refusalView(err, address)
        if (key) state.extra[key] = v
        else state.views[address] = v
        draw()
        // Subscribe again by itself: 5 s, doubling, at most 60 s.
        var wait = state.retry[slot] || RETRY_FIRST_MS
        state.retry[slot] = Math.min(RETRY_MAX_MS, wait * 2)
        if (state.timers[slot]) clearTimeout(state.timers[slot])
        state.timers[slot] = setTimeout(function () {
          state.timers[slot] = null
          subscribe(address, key)
        }, wait)
      }
      try {
        // A refusal ends a subscription; end ours too before the next one.
        if (typeof state.offs[slot] === 'function') state.offs[slot]()
        state.offs[slot] = k2.sources.subscribe(address, on, fail)
      } catch (e) {
        fail({ code: 'failed', message: (e && e.message) || String(e) })
      }
    }
    k2.connected.then(function () {
      var cfg = k2.config || {}
      state.addresses = addressesOf(cfg)
      if (!state.addresses.length) { draw(); return }
      for (var a = 0; a < state.addresses.length; a++) subscribe(state.addresses[a], null)
      var extra = o.extra || []
      for (var i = 0; i < extra.length; i++) {
        var x = cfg[extra[i]]
        if (typeof x === 'string' && x.trim()) subscribe(x.trim(), extra[i])
      }
      draw()
      setInterval(function () { if (state.addresses.some(function (s) { return state.views[s] })) draw() }, TICK_MS)
    }).catch(function (err) {
      clear(root)
      root.appendChild(el('p', 'k2c-notice', 'K2 didn’t connect this card: ' + ((err && err.message) || err)))
    })
  }

  window.K2Cards = {
    version: 2,
    el: el,
    clear: clear,
    ago: ago,
    clock: clock,
    hostOf: hostOf,
    nameOf: nameOf,
    serverNow: serverNow,
    notice: notice,
    greyed: greyed,
    softHint: softHint,
    worse: worse,
    isView: isView,
    mainItems: mainItems,
    listItems: listItems,
    merge: merge,
    addressesOf: addressesOf,
    header: header,
    statusGrid: statusGrid,
    alertBanner: alertBanner,
    numberTrend: numberTrend,
    sparkPath: sparkPath,
    list: list,
    section: section,
    mount: mount,
  }
})()
