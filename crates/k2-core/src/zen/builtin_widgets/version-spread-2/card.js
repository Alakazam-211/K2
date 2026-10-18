// k2:version-spread@2 (0.46.1): which K2 version each box runs, from one or
// several fleet sources (`config.source` plus `config.sources`, boxes merged
// by key). One bar per version (newest on top), the boxes behind the target
// amber, a box that didn't answer counted as unknown, and a sparkline from
// history.
// The target, first that is set:
//   1. `config.latest`: a source whose `newest` field names the newest
//      release (the `github-latest-release` preset);
//   2. a source's own `target_version` field (K2 Web's ops boxes);
//   3. `config.target` (e.g. "0.46.0");
//   4. the newest version any box runs.
// When a source rates versions itself (0.46.1 `versionBehind` rules and the
// `behind` / `far_behind` aggregates), the card shows those counts and each
// box's own health instead of its own guess.
// Config: source, sources?, latest?, target?, trend? (the history series;
// default "count"), title?.
(function () {
  'use strict'
  var C = window.K2Cards

  /** "v0.45.6" and "0.45.6" sort and compare as the same numbers. */
  function parts(v) {
    return String(v).replace(/^v/i, '').split(/[.+-]/).map(function (p) { return /^\d+$/.test(p) ? Number(p) : p })
  }
  function cmp(a, b) {
    var x = parts(a), y = parts(b)
    for (var i = 0; i < Math.max(x.length, y.length); i++) {
      var p = x[i], q = y[i]
      if (p === undefined) return -1
      if (q === undefined) return 1
      if (p === q) continue
      if (typeof p === 'number' && typeof q === 'number') return p < q ? -1 : 1
      return String(p) < String(q) ? -1 : 1
    }
    return 0
  }
  function str(v) { return typeof v === 'string' && v.trim() ? v.trim() : null }
  /** A box that didn't answer: no version, unknown, or red with a fetch
   *  reason (a box a version rule rates red still has its version). */
  function down(it) {
    var v = it.fields ? str(it.fields.version) : null
    return v === null || it.health === 'unknown' || (it.health === 'crit' && !!it.reason)
  }
  function num(v) { return typeof v === 'number' && isFinite(v) ? v : null }

  var trend = { key: null, points: null, asked: 0 }

  C.mount({
    title: 'Version spread',
    extra: ['latest'],
    render: function (main, ctx) {
      var view = ctx.view
      var cfg = ctx.config || {}
      var latest = ctx.extra.latest
      var newest = latest && latest.fields ? str(latest.fields.newest) : null
      var sourceTarget = view.fields ? str(view.fields.target_version) : null
      var items = C.mainItems(view)
      if (view.keep === 'states') {
        C.list(main, [], { empty: 'This source keeps states only, so it has no versions to show.' })
        return
      }
      var counts = {}
      var unknown = 0
      for (var i = 0; i < items.length; i++) {
        var it = items[i]
        if (down(it)) { unknown += 1; continue }
        var v = str(it.fields.version)
        counts[v] = (counts[v] || 0) + 1
      }
      var versions = Object.keys(counts).sort(function (a, b) { return -cmp(a, b) })
      var target = newest || sourceTarget || str(cfg.target) || versions[0] || null
      var targetFrom = newest ? 'Newest release' : sourceTarget ? 'Fleet target' : str(cfg.target) ? 'Target' : 'Newest seen'
      var behind = 0
      for (var j = 0; j < versions.length; j++) if (target && cmp(versions[j], target) < 0) behind += counts[versions[j]]
      var rated = view.fields ? num(view.fields.behind) : null
      var far = view.fields ? num(view.fields.far_behind) : null
      if (rated !== null && !newest) behind = rated
      var series = str(cfg.trend) || 'count'
      if (trend.key !== ctx.address + '|' + series && trend.asked < 3) {
        trend.key = ctx.address + '|' + series
        trend.asked += 1
        k2.sources.history(ctx.address, series).then(function (h) {
          trend.points = h && Array.isArray(h.points) ? h.points : null
          ctx.redraw()
        }).catch(function () { trend.points = null })
      }
      var boxes = items.length - unknown
      C.numberTrend(main, {
        value: versions.length,
        label: (versions.length === 1 ? 'version across ' : 'versions across ') + boxes + ' boxes',
        state: far ? 'crit' : behind ? 'warn' : 'ok',
        points: trend.points,
      })
      var note = target ? targetFrom + ': ' + target : 'No versions reported yet.'
      if (behind) note += ' · ' + behind + ' behind'
      if (far) note += ' · ' + far + ' far behind'
      if (unknown) note += ' · ' + unknown + ' unknown'
      if (latest && latest.reach && latest.reach !== 'ok') note += ' · newest release unknown (' + String(latest.reach).replace(/_/g, ' ') + ')'
      var p = C.el('p', 'k2c-empty', note)
      p.setAttribute('data-k2c-target', target || '')
      main.appendChild(p)
      var bars = C.el('div', 'k2c-bars')
      var max = 0
      for (var k = 0; k < versions.length; k++) max = Math.max(max, counts[versions[k]])
      for (var m = 0; m < versions.length; m++) {
        var ver = versions[m]
        var st = !target || cmp(ver, target) >= 0 ? 'ok' : 'warn'
        var bar = C.el('div', 'k2c-bar k2c-' + st)
        bar.setAttribute('data-k2c-version', ver)
        bar.appendChild(C.el('span', 'k2c-row-main', ver))
        var fill = C.el('div', 'k2c-bar-fill')
        fill.style.width = Math.max(4, Math.round((counts[ver] / max) * 100)) + '%'
        bar.appendChild(fill)
        bar.appendChild(C.el('span', 'k2c-row-side', String(counts[ver])))
        bars.appendChild(bar)
      }
      main.appendChild(bars)
      // The boxes behind (and the unknown ones), as tiles to ask about. A box
      // the source already rates warn or crit keeps that health.
      var lagging = items.filter(function (it) {
        return down(it) || it.health === 'warn' || it.health === 'crit' || (target && cmp(str(it.fields.version), target) < 0)
      }).map(function (it) {
        var h = down(it) ? (it.health === 'crit' ? 'crit' : 'unknown') : C.worse('warn', it.health)
        return { key: it.key, health: h, reason: it.reason, soft: it.soft, fields: it.fields, source: it.source }
      })
      if (lagging.length) C.statusGrid(main, lagging, { detailFields: ['version'], stale: ctx.stale, ask: ctx.ask })
    },
  })
})()
