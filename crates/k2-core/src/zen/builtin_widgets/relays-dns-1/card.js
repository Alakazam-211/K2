// k2:relays-dns@1 (Garden v3, contract-sources-v1 §11.4): relays and DNS
// health. Two sources work:
//   - K2 Web's `k2-fleet-summary`: boxes carry `relay` (and `up`); the card
//     groups them by relay, a relay red when any box on it is down;
//   - a Gatus endpoint (`gatus` preset): one tile per check, grouped by its
//     `group` (`up` true/false).
// With neither (a plain `k2-fleet` fan-out has no relay field), the card
// says "Needs K2 Web's fleet-summary or a Gatus endpoint".
// Config: source, title?.
(function () {
  'use strict'
  var C = window.K2Cards
  var NEEDS = 'Needs K2 Web’s fleet-summary or a Gatus endpoint.'

  function has(items, field) {
    for (var i = 0; i < items.length; i++) if (items[i].fields && items[i].fields[field] !== undefined) return true
    return false
  }

  C.mount({
    title: 'Relays & DNS',
    render: function (main, ctx) {
      var items = ctx.view.items
      if (ctx.view.keep === 'states' && items.length) {
        C.statusGrid(main, items, { stale: ctx.stale, ask: ctx.ask })
        return
      }
      if (has(items, 'relay')) {
        // fleet-summary: one tile per relay.
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
          var rank = { ok: 0, unknown: 1, warn: 2, crit: 3 }
          var h = down ? 'crit' : it.health
          if (rank[h] > rank[g.worst]) g.worst = h
        }
        var tiles = order.map(function (k) {
          var g = relays[k]
          return { key: k, health: g.worst, reason: null, soft: null, fields: { boxes: g.boxes + ' boxes', down: g.down ? g.down + ' down' : null } }
        })
        C.alertBanner(main, ctx.view, { okText: 'All relays healthy' })
        C.statusGrid(main, tiles, { detailFields: ['boxes', 'down'], stale: ctx.stale, ask: ctx.ask })
        return
      }
      if (has(items, 'up')) {
        // Gatus: one tile per check, groups in order.
        C.alertBanner(main, ctx.view, { okText: 'All checks passing', critWord: 'failing' })
        var groups = {}
        var names = []
        for (var j = 0; j < items.length; j++) {
          var grp = items[j].fields.group === null || items[j].fields.group === undefined ? '' : String(items[j].fields.group)
          if (!groups[grp]) { groups[grp] = []; names.push(grp) }
          groups[grp].push(items[j])
        }
        for (var n = 0; n < names.length; n++) {
          if (names[n]) main.appendChild(C.el('h2', 'k2c-row-side', names[n]))
          C.statusGrid(main, groups[names[n]], { labelField: 'name', stale: ctx.stale, ask: ctx.ask })
        }
        return
      }
      var p = C.el('p', 'k2c-notice', NEEDS)
      p.setAttribute('data-k2c-needs', '')
      main.appendChild(p)
    },
  })
})()
