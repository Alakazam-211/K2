// k2:fleet-grid@1 (Garden v3, contract-sources-v1 §11.4): one tile per box
// of a fleet source (`k2-fleet` fan-out over each box's /boot-status, or
// K2 Web's `k2-fleet-summary`; same item keys and fields). A down box is a
// red tile with its reason (GV55), never a grey card; unknown has its own
// colour. Red, stale and unknown tiles offer "Ask an agent".
// Config: source, title?, labelField? (default the item key),
// detailFields? (comma separated; default "version, phase").
(function () {
  'use strict'
  var C = window.K2Cards
  C.mount({
    title: 'Fleet',
    render: function (main, ctx) {
      var cfg = ctx.config || {}
      var detail = typeof cfg.detailFields === 'string' && cfg.detailFields.trim()
        ? cfg.detailFields.split(',').map(function (s) { return s.trim() }).filter(Boolean)
        : ['version', 'phase']
      C.alertBanner(main, ctx.view, { okText: 'All ' + ctx.view.items.length + ' boxes ready' })
      C.statusGrid(main, ctx.view.items, {
        labelField: typeof cfg.labelField === 'string' && cfg.labelField ? cfg.labelField : null,
        detailFields: detail,
        stale: ctx.stale,
        ask: ctx.ask,
        empty: 'No boxes in this source yet.',
      })
    },
  })
})()
