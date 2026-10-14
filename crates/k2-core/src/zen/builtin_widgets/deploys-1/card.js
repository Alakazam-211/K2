// k2:deploys@1 (Garden v3, contract-sources-v1 §11.4): the newest
// production deployments from the `vercel-deployments` preset, one row
// each: its state (READY green, BUILDING / QUEUED amber, ERROR red), the
// deployment's host and how long ago it was made (from the server's
// clock). A failed deploy offers "Ask an agent".
// Config: source, rows? (1 to 20; default 5), title?.
(function () {
  'use strict'
  var C = window.K2Cards

  /** Vercel's `created` is milliseconds; anything that big is ms. */
  function secs(t) {
    if (typeof t !== 'number' || !isFinite(t)) return null
    return t > 1e12 ? t / 1000 : t
  }

  C.mount({
    title: 'Deploys',
    render: function (main, ctx) {
      var cfg = ctx.config || {}
      var n = Number(cfg.rows)
      var rows = isFinite(n) && n >= 1 ? Math.min(20, Math.floor(n)) : 5
      var now = C.serverNow(ctx.view)
      var items = ctx.view.items.slice().sort(function (a, b) {
        return (secs(b.fields && b.fields.created) || 0) - (secs(a.fields && a.fields.created) || 0)
      }).slice(0, rows)
      C.alertBanner(main, ctx.view, { okText: 'Latest deploys ready', critWord: 'failed', warnWord: 'building' })
      C.list(main, items.map(function (it) {
        var f = it.fields || {}
        var state = typeof f.state === 'string' ? f.state : it.health
        var created = secs(f.created)
        var host = typeof f.url === 'string' && f.url ? f.url.replace(/^https?:\/\//, '') : it.key
        var side = [String(state).toLowerCase()]
        if (created !== null && now !== null) side.push(C.ago(now - created))
        return { key: it.key, state: it.health, primary: host, secondary: side.join(' · ') }
      }), { stale: ctx.stale, ask: ctx.ask, empty: 'No production deployments yet.' })
    },
  })
})()
