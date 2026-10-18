// K2's card kit for data-source cards (Garden v3, contract-sources-v1
// §11.4, PRD §12). Every built-in card includes this file byte for byte
// (`include_str!`), so each card version stays one flat, immutable file set.
//
// What every card gets from it:
//   - one source subscription (`k2.sources.subscribe(k2.config.source)`),
//     and optionally a second (`config.latest`), with the refusals drawn
//     as states: not saved, can't read, removed, unreachable (the last view
//     greyed);
//   - the header: title, a state chip, "updated N ago" from the server's
//     clock (serverNow - updatedAt, advanced by local elapsed time, never
//     the viewer's clock), "retrying 2/3", "recovered N ago" for an hour,
//     muted and TEST badges, the link's host;
//   - the notice line: waiting for the first check, stale with the reason,
//     "Paused while this computer slept (02:10–07:42)", "K2 restarted",
//     "<host> unreachable", "Add <host> to your servers", "Removed on …";
//   - renderers: status grid (tiles; unknown has its own colour), alert
//     banner, number + trend (a sparkline from history), and list;
//   - "Ask an agent" on red, stale and unknown tiles (`k2.sources.ask`):
//     K2 writes the question; the card supplies no text.
// Never blank after the first load. Agent and source text only ever goes
// through textContent; no inline handlers; no network or storage.
(function () {
  'use strict'

  var TICK_MS = 15000
  var HOUR = 3600

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

  /** "soft crit 2/3" (0.46.1: the reading waiting to go hard), or the
   *  older "retrying 2/3" when the daemon sends no level; '' when nothing
   *  waits. */
  function softHint(soft) {
    if (!soft || !(soft.count > 0)) return ''
    if (soft.level) return 'soft ' + soft.level + ' ' + soft.count + '/' + soft.of
    return soft.count < soft.of ? 'retrying ' + soft.count + '/' + soft.of : ''
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

  function linkHost(link) {
    var m = /^https?:\/\/([^/?#:]+)/i.exec(String(link || ''))
    return m ? m[1] : null
  }

  var WORDS = { ok: 'OK', warn: 'Warning', crit: 'Down', unknown: 'Unknown', stale: 'Stale', pending: 'Waiting', paused: 'Paused' }

  /** The CSS state for a view or item: its display, greyed when not live. */
  function stateOf(display) {
    return WORDS[display] ? display : 'unknown'
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
      var softText = softHint(view.soft)
      if (softText) meta.appendChild(el('span', 'k2c-soft', softText))
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
      var lh = linkHost(view.link)
      if (lh) {
        var l = el('span', 'k2c-link', '↗ ' + lh)
        l.title = 'Open it from the card’s ⋯ menu'
        meta.appendChild(l)
      }
    }
    root.appendChild(meta)
    var n = notice(view, opts.address)
    if (n) {
      var nb = el('p', 'k2c-notice k2c-notice-' + n.kind, n.text)
      nb.setAttribute('role', 'status')
      root.appendChild(nb)
    }
  }

  // ── renderers ─────────────────────────────────────────────────────

  /** Tiles, one per item. `opts.labelField`, `opts.detailFields`, `opts.ask`. */
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
      if (it.reason) detail.push(it.reason.replace(/_/g, ' '))
      var itSoft = softHint(it.soft)
      if (itSoft) detail.push(itSoft)
      if (detail.length) tile.appendChild(el('div', 'k2c-tile-detail', detail.join(' · ')))
      if (opts.ask && (st === 'crit' || st === 'stale' || st === 'unknown')) {
        var b = el('button', 'k2c-ask', 'Ask an agent')
        b.type = 'button'
        b.setAttribute('data-k2c-ask', it.key)
        b.addEventListener('click', (function (key) {
          return function () { opts.ask(key) }
        })(it.key))
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
    for (var i = 0; i < points.length; i++) if (points[i].v !== null && points[i].v !== undefined) vs.push(points[i].v)
    if (vs.length < 2) return ''
    var lo = Math.min.apply(null, vs), hi = Math.max.apply(null, vs)
    var t0 = points[0].t, t1 = points[points.length - 1].t
    var span = t1 - t0 || 1, rng = hi - lo || 1
    var d = '', pen = false
    for (var j = 0; j < points.length; j++) {
      var p = points[j]
      if (p.v === null || p.v === undefined) { pen = false; continue }
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
    if (o.points && o.points.length > 1) {
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

  /** Rows [{key, state, primary, secondary}]. */
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
        b.addEventListener('click', (function (key) {
          return function () { opts.ask(key) }
        })(r.key))
        li.appendChild(b)
      }
      ul.appendChild(li)
    }
    if (!rows.length) ul.appendChild(el('li', 'k2c-empty', opts.empty || 'Nothing yet.'))
    root.appendChild(ul)
    return ul
  }

  // ── wiring ────────────────────────────────────────────────────────

  /** A refusal drawn as a state: a view-shaped object with a reach. */
  function refusalView(err, address) {
    var code = err && err.code
    if (err && err.last) {
      var last = err.last
      last.__receivedAt = Date.now()
      return last
    }
    var reach = code === 'server_not_saved' || code === 'cap_not_granted' || code === 'removed' || code === 'unreachable' ? code : null
    if (reach) return { address: address, reach: reach }
    return { address: address, reach: 'failed', __error: (err && err.message) || 'K2 couldn’t read this source.' }
  }

  /**
   * mount({render(main, ctx), extra?: ['latest']}): subscribe to
   * config.source (and the extra config keys), call render on every change
   * and every 15 s (ages), and tell K2 ready after the first draw.
   * ctx: {view, extra: {latest: view}, address, config, stale, ask(itemKey), redraw()}.
   */
  function mount(o) {
    var root = document.getElementById('card')
    if (!root) throw new Error('card: missing #card')
    var state = { view: null, extra: {}, address: null, readied: false }
    var draw = function () {
      clear(root)
      var view = state.view
      var ctx = {
        view: view,
        extra: state.extra,
        address: state.address,
        config: k2.config,
        stale: !!(view && greyed(view)),
        ask: function (key) {
          k2.sources.ask(state.address, key === undefined ? null : key).catch(function (e) { console.warn('card: ask failed', e) })
        },
        redraw: function () { draw() },
      }
      if (!state.address) {
        root.appendChild(el('p', 'k2c-notice', 'This card needs a source: set config.source in the Garden file (name, or name::host).'))
      } else if (view && view.__error) {
        header(root, null, { title: o.title || state.address, address: state.address })
        root.appendChild(el('p', 'k2c-notice', view.__error))
      } else if (!view) {
        header(root, null, { title: o.title, address: state.address })
        root.appendChild(el('p', 'k2c-notice', 'Loading…'))
      } else if (view.serverNow === undefined) {
        // A refusal with no last view: the notice alone, never a blank card.
        header(root, null, { title: (k2.config && k2.config.title) || state.address, address: state.address })
        var n = notice(view, state.address)
        root.appendChild(el('p', 'k2c-notice', n ? n.text : 'K2 can’t read this source.'))
      } else {
        var main = el('main', 'k2c-body' + (ctx.stale ? ' k2c-greyed' : ''))
        header(root, view, { title: (k2.config && k2.config.title) || o.title, address: state.address })
        root.appendChild(main)
        o.render(main, ctx)
      }
      if (!state.readied) {
        state.readied = true
        k2.ready()
      }
    }
    k2.connected.then(function () {
      var cfg = k2.config || {}
      state.address = typeof cfg.source === 'string' && cfg.source ? cfg.source.split(',')[0].trim() : null
      if (!state.address) { draw(); return }
      var on = function (key) {
        return function (v) {
          v.__receivedAt = Date.now()
          if (key) state.extra[key] = v
          else state.view = v
          draw()
        }
      }
      var fail = function (key, address) {
        return function (err) {
          var v = refusalView(err, address)
          if (key) state.extra[key] = v
          else state.view = v
          draw()
        }
      }
      k2.sources.subscribe(state.address, on(null), fail(null, state.address))
      var extra = o.extra || []
      for (var i = 0; i < extra.length; i++) {
        var a = cfg[extra[i]]
        if (typeof a === 'string' && a) k2.sources.subscribe(a.trim(), on(extra[i]), fail(extra[i], a.trim()))
      }
      draw()
      setInterval(function () { if (state.view) draw() }, TICK_MS)
    }).catch(function (err) {
      clear(root)
      root.appendChild(el('p', 'k2c-notice', 'K2 didn’t connect this card: ' + ((err && err.message) || err)))
    })
  }

  window.K2Cards = {
    el: el,
    clear: clear,
    ago: ago,
    clock: clock,
    hostOf: hostOf,
    serverNow: serverNow,
    notice: notice,
    greyed: greyed,
    header: header,
    statusGrid: statusGrid,
    alertBanner: alertBanner,
    numberTrend: numberTrend,
    sparkPath: sparkPath,
    list: list,
    mount: mount,
  }
})()
