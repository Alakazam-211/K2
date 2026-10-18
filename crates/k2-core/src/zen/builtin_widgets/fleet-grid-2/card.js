// k2:fleet-grid@2 (0.46.1): one tile per box, from one or several fleet
// sources (`config.source` plus `config.sources`): K2's `k2-fleet` fan-out
// over each box's /boot-status, K2 Web's `k2-fleet-summary`, or K2 Web's
// ops boxes (`/api/ops/fleet?part=boxes`, items keyed by label). Boxes with
// the same key across sources are one tile (the worst health wins, fields
// filled in source order), so `source = "fleet"`, `sources = "k2-ops-boxes"`
// shows each box's version and phase plus whose go it needs.
// A down box is a red tile with its reason (GV55), never a grey card;
// unknown has its own colour; a reading waiting to go hard says
// "soft crit 2/3". Red, stale and unknown tiles offer "Ask an agent".
// Config: source, sources?, title?, labelField? (default the item key),
// detailFields? (comma separated; default "version, phase"), list? (a named
// list, `extract.lists`, instead of the boxes).
(function () {
  'use strict'
  var C = window.K2Cards

  /** customer / needs_go / online from K2 Web's ops boxes. */
  function marks(it) {
    var f = it.fields || {}
    var out = []
    if (f.customer === true) out.push('customer')
    if (typeof f.needs_go === 'string' && f.needs_go) out.push('go: ' + f.needs_go)
    if (f.online === false && it.health !== 'crit') out.push('offline')
    return out
  }

  C.mount({
    title: 'Fleet',
    render: function (main, ctx) {
      var cfg = ctx.config || {}
      var detail = typeof cfg.detailFields === 'string' && cfg.detailFields.trim()
        ? cfg.detailFields.split(',').map(function (s) { return s.trim() }).filter(Boolean)
        : ['version', 'phase']
      var listName = typeof cfg.list === 'string' && cfg.list.trim() ? cfg.list.trim() : null
      var items = listName ? C.listItems(ctx.view, listName) : C.mainItems(ctx.view)
      var shown = Object.assign({}, ctx.view, { counts: countsOf(items) })
      C.alertBanner(main, shown, { okText: 'All ' + items.length + (listName ? ' ' + listName : ' boxes') + (listName ? ' OK' : ' ready') })
      C.statusGrid(main, items, {
        labelField: typeof cfg.labelField === 'string' && cfg.labelField ? cfg.labelField : null,
        detailFields: detail,
        marks: marks,
        stale: ctx.stale,
        ask: ctx.ask,
        empty: listName ? 'Nothing in ' + listName + ' yet.' : 'No boxes in this source yet.',
      })
    },
  })

  function countsOf(items) {
    var c = { ok: 0, warn: 0, crit: 0, unknown: 0 }
    for (var i = 0; i < items.length; i++) {
      var h = items[i].health
      c[c[h] === undefined ? 'unknown' : h] += 1
    }
    return c
  }
})()
