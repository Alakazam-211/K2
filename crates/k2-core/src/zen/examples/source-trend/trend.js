// Source Trend: one data source with K2's 0.46.1 widget SDK.
// - A handler-object subscription: K2 re-subscribes on its own after a
//   retryable refusal (not_bound until the placement binds it, unreachable,
//   rate_limited) and hands `data` a normalised card view.
// - A pinned chart library (requires.libs: uplot@1.6.32), inlined offline.
// - k2.shape guards for anything else read from K2.
// Text from a source always goes in with textContent, never innerHTML.
(function () {
  const $ = (id) => document.getElementById(id)
  let chart = null

  function setState(text, state) {
    $('state').textContent = text
    $('state').setAttribute('data-state', state)
  }

  function note(text) {
    $('note').hidden = !text
    $('note').textContent = text || ''
  }

  function drawView(view) {
    $('title').textContent = view.label || view.name || 'Source'
    setState(view.display, view.display)
    const list = $('items')
    list.textContent = ''
    for (const it of view.items.slice(0, 50)) {
      const li = document.createElement('li')
      li.setAttribute('data-health', it.health)
      const key = document.createElement('span')
      key.textContent = it.key
      const val = document.createElement('span')
      const soft = it.soft.level ? ' · soft ' + it.soft.level + ' ' + it.soft.count + '/' + it.soft.of : ''
      val.textContent = it.health + soft
      li.append(key, val)
      list.append(li)
    }
  }

  function drawHistory(points) {
    const xs = []
    const ys = []
    for (const p of k2.shape.list(points)) {
      const o = k2.shape.obj(p)
      xs.push(k2.shape.num(o.t))
      ys.push(typeof o.v === 'number' ? o.v : null)
    }
    if (xs.length < 2 || typeof uPlot !== 'function') return
    const el = $('chart')
    const accent = getComputedStyle(document.documentElement).getPropertyValue('--zen-accent').trim() || '#2a7'
    const opts = {
      width: Math.max(120, el.clientWidth || 240),
      height: 60,
      legend: { show: false },
      cursor: { show: false },
      axes: [{ show: false }, { show: false }],
      series: [{}, { stroke: accent, width: 1.5 }],
    }
    if (chart) chart.destroy()
    chart = new uPlot(opts, [xs, ys], el)
  }

  k2.connected.then(() => {
    const source = k2.shape.str(k2.config.source)
    const series = k2.shape.str(k2.config.series)
    if (!source) {
      note('Set config.source to a data source (k2 sources list).')
      k2.ready()
      return
    }
    k2.sources.subscribe(source, {
      data: (view) => {
        note('')
        drawView(view)
        if (series) k2.sources.history(source, series).then((h) => drawHistory(k2.shape.obj(h).points), () => undefined)
      },
      error: (err) => {
        if (err.last) drawView(err.last)
        if (err.retrying) {
          setState('retrying', 'stale')
          note(err.code === 'not_bound' ? 'Waiting for this card’s config to name ' + source + '.' : 'Trying again: ' + err.code)
        } else {
          setState(err.code, 'crit')
          note('Stopped: ' + err.code)
        }
      },
    })
    k2.ready()
  })
})()
