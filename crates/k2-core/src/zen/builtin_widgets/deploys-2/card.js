// k2:deploys@2 (0.46.1): the newest production deployments, from one or
// several sources (`config.source` plus `config.sources`): the
// `vercel-deployments` preset (one row per deployment: READY green,
// BUILDING / QUEUED amber, ERROR red, its host and how long ago), and K2
// Web's ops deploys fields (`web_sha`, `cp_sha_*`: what each part runs).
// A failed deploy offers "Ask an agent".
// Config: source, sources?, rows? (1 to 20; default 5), title?.
(function () {
  'use strict'
  var C = window.K2Cards

  /** Vercel's `created` is milliseconds; anything that big is ms. */
  function secs(t) {
    if (typeof t !== 'number' || !isFinite(t)) return null
    return t > 1e12 ? t / 1000 : t
  }

  function sha(v) {
    return typeof v === 'string' && v ? v.slice(0, 7) : null
  }

  C.mount({
    title: 'Deploys',
    render: function (main, ctx) {
      var cfg = ctx.config || {}
      var n = Number(cfg.rows)
      var rows = isFinite(n) && n >= 1 ? Math.min(20, Math.floor(n)) : 5
      var now = C.serverNow(ctx.view)
      var all = C.mainItems(ctx.view).filter(function (it) { return it.fields && (it.fields.state !== undefined || it.fields.created !== undefined) })
      var items = all.slice().sort(function (a, b) {
        return (secs(b.fields.created) || 0) - (secs(a.fields.created) || 0)
      }).slice(0, rows)
      var f = ctx.view.fields || {}
      var shas = []
      if (sha(f.web_sha)) shas.push({ key: 'web', state: 'ok', primary: 'web', secondary: sha(f.web_sha) })
      for (var k in f) {
        if (Object.prototype.hasOwnProperty.call(f, k) && /^cp_sha_/.test(k) && sha(f[k])) {
          shas.push({ key: k, state: 'ok', primary: 'control plane ' + k.slice(7).replace(/^k2e(\d)/, 'k2e-$1'), secondary: sha(f[k]) })
        }
      }
      if (all.length || !shas.length) {
        var counts = { ok: 0, warn: 0, crit: 0, unknown: 0 }
        for (var i = 0; i < all.length; i++) counts[counts[all[i].health] === undefined ? 'unknown' : all[i].health] += 1
        C.alertBanner(main, Object.assign({}, ctx.view, { counts: counts }), { okText: 'Latest deploys ready', critWord: 'failed', warnWord: 'building' })
        C.list(main, items.map(function (it) {
          var fl = it.fields
          var state = typeof fl.state === 'string' ? fl.state : it.health
          var created = secs(fl.created)
          var host = typeof fl.url === 'string' && fl.url ? fl.url.replace(/^https?:\/\//, '') : it.key
          var side = [String(state).toLowerCase()]
          if (created !== null && now !== null) side.push(C.ago(now - created))
          var sh = C.softHint(it.soft)
          if (sh) side.push(sh)
          return { key: it.key, state: it.health, primary: host, secondary: side.join(' · '), source: it.source }
        }), { stale: ctx.stale, ask: ctx.ask, empty: 'No production deployments yet.' })
      }
      if (shas.length) {
        C.section(main, 'Running')
        C.list(main, shas, { stale: ctx.stale })
      }
    },
  })
})()
