// k2:relays-dns@2 (0.46.1): relays, DNS and the edge's own checks, from one
// or several sources (`config.source` plus `config.sources`). What each
// shape draws:
//   - K2 Web's `k2-fleet-summary`: boxes carry `relay` (and `up`); one tile
//     per relay, red when any box on it is down;
//   - K2 Web's ops edge (`/api/ops/fleet?part=relays,dns,deploys`): relay
//     items with `role` / `tunnels` / service states; one tile per relay
//     with its role and tunnels, coloured by the source's own rules; the DNS
//     fields (`wildcard_a`, `failover_armed`) as one row;
//   - a Gatus endpoint (`gatus` preset): one tile per check, by `group`;
//   - a status-only source (0.46.1 `expect.status`, no items, e.g. k2.dev or
//     connect.k2.dev): one tile with its state and HTTP status.
// With none of these, the card says what it needs.
// Config: source, sources?, title?.
(function () {
  'use strict'
  var C = window.K2Cards
  var NEEDS = 'Needs K2 Web’s fleet-summary or ops edge, a Gatus endpoint, or a status check.'
  var RANK = { ok: 0, unknown: 1, warn: 2, crit: 3 }

  function has(items, field) {
    for (var i = 0; i < items.length; i++) if (items[i].fields && items[i].fields[field] !== undefined) return true
    return false
  }

  function byRelay(items) {
    var relays = {}
    var order = []
    for (var i = 0; i < items.length; i++) {
      var it = items[i]
      var r = it.fields.relay === null || it.fields.relay === undefined || it.fields.relay === '' ? 'no relay' : String(it.fields.relay)
      if (!relays[r]) { relays[r] = { key: r, boxes: 0, down: 0, worst: 'ok' }; order.push(r) }
      var g = relays[r]
      g.boxes += 1
      var down = it.health === 'crit' || it.fields.up === false
      if (down) g.down += 1
      var h = down ? 'crit' : it.health
      if (RANK[h] > RANK[g.worst]) g.worst = h
    }
    return order.map(function (k) {
      var g = relays[k]
      return { key: k, health: g.worst, reason: null, soft: null, fields: { boxes: g.boxes + ' boxes', down: g.down ? g.down + ' down' : null } }
    })
  }

  C.mount({
    title: 'Relays & DNS',
    render: function (main, ctx) {
      var view = ctx.view
      var items = C.mainItems(view)
      var drew = false
      if (view.keep === 'states' && items.length) {
        C.statusGrid(main, items, { stale: ctx.stale, ask: ctx.ask })
        drew = true
      } else {
        var relayBoxes = items.filter(function (it) { return it.fields && it.fields.relay !== undefined })
        var edge = items.filter(function (it) { return it.fields && (it.fields.role !== undefined || it.fields.tunnels !== undefined) })
        var gatus = items.filter(function (it) { return it.fields && it.fields.up !== undefined && it.fields.relay === undefined })
        if (relayBoxes.length || edge.length || gatus.length) {
          C.alertBanner(main, view, { okText: 'All relays healthy' })
          drew = true
        }
        if (relayBoxes.length) C.statusGrid(main, byRelay(relayBoxes), { detailFields: ['boxes', 'down'], stale: ctx.stale, ask: ctx.ask })
        if (edge.length) {
          C.statusGrid(main, edge.map(function (it) {
            var f = it.fields
            var t = typeof f.tunnels === 'number' ? f.tunnels + ' tunnels' : null
            return { key: it.key, health: it.health, reason: it.reason, soft: it.soft, source: it.source, fields: { role: f.role, tunnels: t } }
          }), { detailFields: ['role', 'tunnels'], stale: ctx.stale, ask: ctx.ask })
        }
        if (gatus.length) {
          var groups = {}
          var names = []
          for (var j = 0; j < gatus.length; j++) {
            var grp = gatus[j].fields.group === null || gatus[j].fields.group === undefined ? '' : String(gatus[j].fields.group)
            if (!groups[grp]) { groups[grp] = []; names.push(grp) }
            groups[grp].push(gatus[j])
          }
          for (var n = 0; n < names.length; n++) {
            if (names[n]) C.section(main, names[n])
            C.statusGrid(main, groups[names[n]], { labelField: 'name', stale: ctx.stale, ask: ctx.ask })
          }
        }
        var f = view.fields || {}
        if (f.wildcard_a !== undefined || f.failover_armed !== undefined) {
          var dnsState = f.wildcard_a === 'other' ? 'crit' : f.wildcard_a === 'standby' || f.failover_armed === false ? 'warn' : f.wildcard_a === 'primary' ? 'ok' : 'unknown'
          var side = []
          if (f.failover_armed === true) side.push('failover armed')
          if (f.failover_armed === false) side.push('failover not armed')
          C.list(main, [{ key: 'dns', state: dnsState, primary: 'DNS: ' + (f.wildcard_a === null || f.wildcard_a === undefined ? 'unknown' : String(f.wildcard_a)), secondary: side.join(' · ') }], { stale: ctx.stale })
          drew = true
        }
      }
      // Status-only sources: one tile each.
      var checks = ctx.views.filter(function (s) {
        return C.isView(s.view) && !(Array.isArray(s.view.items) && s.view.items.length) && (!s.view.fields || Object.keys(s.view.fields).length === 0)
      })
      if (checks.length) {
        if (!drew) C.alertBanner(main, Object.assign({}, view, { counts: countChecks(checks) }), { okText: 'All checks passing', critWord: 'failing' })
        C.statusGrid(main, checks.map(function (s) {
          var v = s.view
          return {
            key: C.nameOf(s.address),
            health: C.greyed(v) ? 'unknown' : v.health,
            reason: v.reason,
            soft: v.soft,
            source: s.address,
            fields: { http: typeof v.httpStatus === 'number' ? 'HTTP ' + v.httpStatus : null },
          }
        }), { labelField: null, detailFields: ['http'], stale: false, ask: function (key, address) { ctx.ask(null, address) } })
        drew = true
      }
      if (!drew) {
        var p = C.el('p', 'k2c-notice', NEEDS)
        p.setAttribute('data-k2c-needs', '')
        main.appendChild(p)
      }
    },
  })

  function countChecks(checks) {
    var c = { ok: 0, warn: 0, crit: 0, unknown: 0 }
    for (var i = 0; i < checks.length; i++) {
      var h = checks[i].view.health
      c[c[h] === undefined ? 'unknown' : h] += 1
    }
    return c
  }
})()
