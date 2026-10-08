// Hello: a clock and a hello to the first agent this widget can see.
// Uses K2's `k2` object (k2 zen guide api). Agent text always goes in
// with textContent, never innerHTML.
(function () {
  const clock = document.getElementById('clock')
  const greeting = document.getElementById('greeting')

  function tick() {
    const now = new Date()
    const hh = String(now.getHours()).padStart(2, '0')
    const mm = String(now.getMinutes()).padStart(2, '0')
    clock.textContent = hh + ':' + mm
  }
  tick()
  setInterval(tick, 1000)

  function greet(rows) {
    const first = rows && rows[0]
    greeting.textContent = first ? 'Hello, ' + first.label + '.' : 'Hello.'
  }

  if (k2.can('agents.subscribe')) {
    k2.agents.subscribe(greet)
  }
  k2.ready()
})()
