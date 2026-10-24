// k2:money-alerts@3 (0.46.4): @2 without "Ask an agent" (sources-kit-3).
// @2 (0.46.1): money that needs a person, from one or several
// sources (`config.source` plus `config.sources`):
//   - the `stripe-past-due` preset: states only by default (GV47), so red
//     "Past-due subscriptions found" or green "No past-due subscriptions"
//     and nothing else; with `keep: values`, the count ("100+" past a page);
//   - K2 Web's ops billing (`/api/ops/fleet?part=billing`): the past-due
//     count and how many aren't kept live, canceled subscriptions still
//     routing (the `canceled` list, or the `canceled_routing` count), and
//     the reconcile; each row coloured by the source's own rules.
// The banner is the worst state of all the sources.
// Config: source, sources?, title?.
(function () {
  'use strict'
  var C = window.K2Cards

  function num(v) { return typeof v === 'number' && isFinite(v) ? v : null }

  C.mount({
    title: 'Money alerts',
    render: function (main, ctx) {
      var view = ctx.view
      var st = ctx.stale ? 'stale' : view.display
      var text = view.phase === 'pending'
        ? 'Waiting for the first check'
        : view.health === 'crit' ? 'Money needs a person now' : view.health === 'warn' ? 'Money to look at' : view.health === 'ok' ? 'Nothing past due' : 'Can’t tell right now'
      var f = view.fields || {}
      var stripe = num(f.pastDue) !== null || view.keep === 'states'
      if (stripe && ctx.views.length === 1) {
        text = view.phase === 'pending' ? 'Waiting for the first check' : view.health === 'crit' ? 'Past-due subscriptions found' : view.health === 'ok' ? 'No past-due subscriptions' : 'Can’t tell right now'
      }
      var banner = C.el('div', 'k2c-banner k2c-' + (st === 'ok' || st === 'crit' || st === 'warn' || st === 'stale' ? st : 'unknown'), text)
      banner.setAttribute('data-k2c-banner', st)
      main.appendChild(banner)
      if (view.keep !== 'states' && num(f.pastDue) !== null) {
        var more = f.hasMore === true
        C.numberTrend(main, { value: more ? '100+' : String(f.pastDue), label: 'past-due subscriptions', state: view.health === 'crit' ? 'crit' : 'ok' })
      } else if (view.keep === 'states' && ctx.views.length === 1) {
        main.appendChild(C.el('p', 'k2c-empty', 'States only: this source keeps no amounts or counts.'))
      }
      // K2 Web's ops billing.
      var pastDue = num(f.past_due)
      var notKept = num(f.past_due_not_kept)
      var canceled = C.listItems(view, 'canceled')
      var canceledN = num(f.canceled_routing)
      if (pastDue !== null || canceled.length || canceledN !== null || f.reconcile_ok !== undefined) {
        var rows = []
        if (pastDue !== null) {
          rows.push({ key: 'past_due', state: notKept ? 'warn' : 'ok', primary: pastDue + ' past due', secondary: notKept ? notKept + ' not kept live' : 'all kept live' })
        }
        var pd = C.mainItems(view).filter(function (it) { return it.fields && it.fields.keep_live !== undefined })
        for (var i = 0; i < pd.length; i++) {
          rows.push({ key: pd[i].key, state: pd[i].health, primary: pd[i].key, secondary: pd[i].fields.keep_live === false ? 'not kept live' : 'kept live', source: pd[i].source })
        }
        if (canceled.length || canceledN) {
          var n = canceled.length || canceledN
          rows.push({ key: 'canceled_routing', state: 'crit', primary: n + ' canceled but still routing', secondary: null })
          for (var j = 0; j < canceled.length; j++) {
            var label = canceled[j].fields && typeof canceled[j].fields.canceled_label === 'string' ? canceled[j].fields.canceled_label : String(canceled[j].key).replace(/^canceled\//, '')
            rows.push({ key: canceled[j].key, state: canceled[j].health === 'ok' ? 'crit' : canceled[j].health, primary: label, secondary: 'canceled, still routing', source: canceled[j].source })
          }
        }
        if (f.reconcile_ok !== undefined && f.reconcile_ok !== null) {
          rows.push({ key: 'reconcile', state: f.reconcile_ok === false ? 'crit' : 'ok', primary: 'Stripe reconcile', secondary: f.reconcile_ok === false ? 'failing' : 'ok' })
        }
        C.list(main, rows, { stale: ctx.stale })
      }
    },
  })
})()
