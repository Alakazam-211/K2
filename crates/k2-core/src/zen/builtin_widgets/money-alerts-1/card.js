// k2:money-alerts@1 (Garden v3, contract-sources-v1 §11.4): past-due
// subscriptions from the `stripe-past-due` preset. By default the source
// keeps states only (GV47: no amounts or counts at rest), so the card says
// red "Past-due subscriptions found" or green "No past-due subscriptions"
// and nothing else. With `keep: values` it shows the count, "100+" when
// Stripe says there are more than one page.
// Config: source, title?.
(function () {
  'use strict'
  var C = window.K2Cards

  C.mount({
    title: 'Money alerts',
    render: function (main, ctx) {
      var view = ctx.view
      var st = ctx.stale ? 'stale' : view.display
      var found = view.health === 'crit'
      var text = view.phase === 'pending' ? 'Waiting for the first check' : found ? 'Past-due subscriptions found' : view.health === 'ok' ? 'No past-due subscriptions' : 'Can’t tell right now'
      var banner = C.el('div', 'k2c-banner k2c-' + (st === 'ok' || st === 'crit' || st === 'warn' || st === 'stale' ? st : 'unknown'), text)
      banner.setAttribute('data-k2c-banner', st)
      main.appendChild(banner)
      if (view.keep !== 'states' && view.fields && typeof view.fields.pastDue === 'number') {
        var more = view.fields.hasMore === true
        C.numberTrend(main, { value: more ? '100+' : String(view.fields.pastDue), label: 'past-due subscriptions', state: found ? 'crit' : 'ok' })
      } else if (view.keep === 'states') {
        main.appendChild(C.el('p', 'k2c-empty', 'States only: this source keeps no amounts or counts.'))
      }
      if (found || st === 'stale' || st === 'unknown') {
        var b = C.el('button', 'k2c-ask', 'Ask an agent')
        b.type = 'button'
        b.setAttribute('data-k2c-ask', '')
        b.addEventListener('click', function () { ctx.ask(null) })
        main.appendChild(b)
      }
    },
  })
})()
