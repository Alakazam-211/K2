// k2:version-spread@1 (Garden v3, contract-sources-v1 §11.4): which K2
// version each box of a fleet source runs. One bar per version (newest on
// top), the boxes behind the newest release amber, a box that didn't answer
// counted as unknown, and a sparkline of the fleet's size from history.
// Config: source (a fleet source: items with a `version` field), latest?
// (a second source address whose `newest` field names the newest release,
// e.g. the `github-latest-release` preset), title?.
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
  function same(a, b) { return cmp(a, b) === 0 }

  var trend = { address: null, points: null, asked: 0 }

  C.mount({
    title: 'Version spread',
    extra: ['latest'],
    render: function (main, ctx) {
      var view = ctx.view
      var latest = ctx.extra.latest
      var newest = latest && latest.fields && typeof latest.fields.newest === 'string' ? latest.fields.newest : null
      var counts = {}
      var unknown = 0
      for (var i = 0; i < view.items.length; i++) {
        var it = view.items[i]
        var v = it.fields ? it.fields.version : null
        if (v === null || v === undefined || v === '' || it.health === 'crit' || it.health === 'unknown') { unknown += 1; continue }
        counts[v] = (counts[v] || 0) + 1
      }
      var versions = Object.keys(counts).sort(function (a, b) { return -cmp(a, b) })
      var top = newest || versions[0] || null
      var behind = 0
      for (var j = 0; j < versions.length; j++) if (top && !same(versions[j], top)) behind += counts[versions[j]]
      if (view.keep === 'states') {
        C.list(main, [], { empty: 'This source keeps states only, so it has no versions to show.' })
        return
      }
      // History: one read per source; no_history is fine.
      if (trend.address !== ctx.address && trend.asked < 3) {
        trend.address = ctx.address
        trend.asked += 1
        k2.sources.history(ctx.address, 'count').then(function (h) { trend.points = h.points; ctx.redraw() }).catch(function () { trend.points = null })
      }
      C.numberTrend(main, {
        value: versions.length,
        label: versions.length === 1 ? 'version across ' + (view.items.length - unknown) + ' boxes' : 'versions across ' + (view.items.length - unknown) + ' boxes',
        state: behind ? 'warn' : 'ok',
        points: trend.points,
      })
      var note = top ? (newest ? 'Newest release: ' + newest : 'Newest seen: ' + top) : 'No versions reported yet.'
      if (behind) note += ' · ' + behind + ' behind'
      if (unknown) note += ' · ' + unknown + ' unknown'
      if (latest && latest.reach && latest.reach !== 'ok') note += ' · newest release unknown (' + latest.reach.replace(/_/g, ' ') + ')'
      main.appendChild(C.el('p', 'k2c-empty', note))
      var bars = C.el('div', 'k2c-bars')
      var max = 0
      for (var k = 0; k < versions.length; k++) max = Math.max(max, counts[versions[k]])
      for (var m = 0; m < versions.length; m++) {
        var ver = versions[m]
        var st = top && same(ver, top) ? 'ok' : 'warn'
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
      // The boxes behind (and the unknown ones), as tiles to ask about.
      var lagging = view.items.filter(function (it) {
        var v = it.fields ? it.fields.version : null
        return it.health === 'crit' || it.health === 'unknown' || v === null || v === undefined || (top && !same(v, top))
      }).map(function (it) {
        var v = it.fields ? it.fields.version : null
        var down = it.health === 'crit' || it.health === 'unknown' || v === null || v === undefined
        return { key: it.key, health: down ? it.health === 'crit' ? 'crit' : 'unknown' : 'warn', reason: it.reason, soft: it.soft, fields: it.fields }
      })
      if (lagging.length) C.statusGrid(main, lagging, { detailFields: ['version'], stale: ctx.stale, ask: ctx.ask })
    },
  })
})()
