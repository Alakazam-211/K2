// Phone check: K2's test kit for Gardens on the phone (prd-gardens-on-phone-v1
// §9, contract-gardens-phone-v1 §11). Place it in a Garden and open that
// Garden in K2 Companion:
//   - "Phone verbs": one row per kind of row in the phone verb table
//     (sdk/generated/phone-verbs.json): door rows answer, host rows answer,
//     later rows answer verb_unavailable, never rows answer not_exposed.
//   - "Ways out of the frame": every escape must read "blocked". "check"
//     means the bridge object is visible and the app must drop what it
//     receives (K2's wry patch UW34 on iOS): look in the app's log for the
//     probe message, which must not arrive.
//   - Canvas: WebKit's toDataURL / toBlob come back empty in a sealed frame
//     (K2's U0 finding); getImageData works. Recorded, not judged.
// It never sends anything off the phone; text goes in with textContent.
(function () {
  const verbs = document.getElementById('verbs')
  const escapes = document.getElementById('escapes')
  const summary = document.getElementById('summary')
  let passed = 0
  let failed = 0

  function row(table, name, outcome, detail) {
    const tr = document.createElement('tr')
    const a = document.createElement('td')
    a.textContent = name
    const b = document.createElement('td')
    b.textContent = outcome
    b.className = outcome === 'PASS' || outcome === 'blocked' ? 'pass' : outcome === 'FAIL' || outcome === 'OPEN' ? 'fail' : ''
    const c = document.createElement('td')
    c.textContent = detail || ''
    tr.append(a, b, c)
    table.append(tr)
    if (b.className === 'pass') passed++
    if (b.className === 'fail') failed++
  }

  // [verb, args, expected: 'ok' | an error code]
  const VERB_CHECKS = [
    ['viewer.get', [], 'ok'],
    ['widget.self', [], 'ok'],
    ['store.put', ['phone-check/last', { at: 'phone' }], 'ok'],
    ['store.get', ['phone-check/last'], 'ok'],
    ['store.list', [{ prefix: 'phone-check/' }], 'ok'],
    ['save.set', ['phone-check', 1], 'ok'],
    ['save.get', ['phone-check'], 'ok'],
    ['theme.get', [], 'ok'],
    ['gardens.list', [], 'ok'],
    ['gardens.current', [], 'ok'],
    ['layout.get', [], 'ok'],
    ['agents.list', [], 'verb_unavailable'],
    ['thread.read', ['k2/1'], 'verb_unavailable'],
    ['window.startDrag', [], 'not_exposed'],
  ]

  async function checkVerbs() {
    for (const [verb, args, want] of VERB_CHECKS) {
      try {
        await k2.call(verb, ...args)
        row(verbs, verb, want === 'ok' ? 'PASS' : 'FAIL', want === 'ok' ? '' : 'answered; want ' + want)
      } catch (e) {
        const code = (e && e.code) || 'failed'
        row(verbs, verb, code === want ? 'PASS' : 'FAIL', code + (e && e.message ? ': ' + e.message : ''))
      }
    }
  }

  function blockedIf(name, fn) {
    try {
      const r = fn()
      row(escapes, name, r ? 'OPEN' : 'blocked', r ? String(r) : '')
    } catch (e) {
      row(escapes, name, 'blocked', (e && e.name) || 'threw')
    }
  }

  function settle(name, start, ms) {
    return new Promise((resolve) => {
      let done = false
      const finish = (outcome, detail) => {
        if (done) return
        done = true
        row(escapes, name, outcome, detail)
        resolve()
      }
      setTimeout(() => finish('blocked', 'no answer in ' + ms + ' ms'), ms)
      try {
        start(finish)
      } catch (e) {
        finish('blocked', (e && e.name) || 'threw')
      }
    })
  }

  async function checkEscapes() {
    const w = /** @type {any} */ (window)
    const bridge = (name, obj) => {
      if (!obj) return row(escapes, name, 'blocked', 'absent')
      try {
        obj.postMessage(JSON.stringify({ k2probe: name }))
      } catch (e) {
        return row(escapes, name, 'blocked', (e && e.name) || 'threw')
      }
      row(escapes, name, 'check', 'visible: the app must drop this message (look for k2probe in its log)')
    }
    bridge('webkit ipc', w.webkit && w.webkit.messageHandlers && w.webkit.messageHandlers.ipc)
    bridge('window.ipc', w.ipc)
    blockedIf('__TAURI_INTERNALS__', () => (typeof w.__TAURI_INTERNALS__ === 'undefined' ? '' : 'present'))
    blockedIf('__TAURI__', () => (typeof w.__TAURI__ === 'undefined' ? '' : 'present'))
    blockedIf('top.location', () => String(top.location.href))
    blockedIf('parent.document', () => (parent.document ? 'readable' : ''))
    blockedIf('document.cookie', () => (document.cookie, 'readable'))
    blockedIf('localStorage', () => (localStorage.length >= 0 ? 'readable' : ''))
    blockedIf('RTCPeerConnection', () => (typeof RTCPeerConnection === 'undefined' ? '' : new RTCPeerConnection() && 'constructed'))
    await settle('fetch', (finish) => {
      fetch('https://example.com/').then(
        () => finish('OPEN', 'fetch answered'),
        (e) => finish('blocked', (e && e.name) || 'rejected'),
      )
    }, 3000)
    await settle('WebSocket', (finish) => {
      const s = new WebSocket('wss://example.com/')
      s.onopen = () => finish('OPEN', 'opened')
      s.onerror = () => finish('blocked', 'error')
      s.onclose = () => finish('blocked', 'closed')
    }, 3000)
  }

  function checkCanvas() {
    const c = document.createElement('canvas')
    c.width = 2
    c.height = 2
    const g = c.getContext('2d')
    if (!g) return row(escapes, 'canvas', 'note', 'no 2d context')
    g.fillStyle = '#ff0000'
    g.fillRect(0, 0, 2, 2)
    let url = ''
    try {
      url = c.toDataURL('image/png')
    } catch (e) {
      url = 'threw ' + ((e && e.name) || '')
    }
    row(escapes, 'canvas.toDataURL', 'note', url.length > 30 ? 'works (' + url.length + ' chars)' : 'empty: ' + url)
    let px = ''
    try {
      px = Array.from(g.getImageData(0, 0, 1, 1).data).join(',')
    } catch (e) {
      px = 'threw ' + ((e && e.name) || '')
    }
    row(escapes, 'canvas.getImageData', 'note', px)
    return new Promise((resolve) => {
      try {
        c.toBlob((b) => {
          row(escapes, 'canvas.toBlob', 'note', b ? 'works (' + b.size + ' bytes)' : 'null')
          resolve()
        })
      } catch (e) {
        row(escapes, 'canvas.toBlob', 'note', 'threw ' + ((e && e.name) || ''))
        resolve()
      }
    })
  }

  k2.connected.then(async () => {
    k2.ready()
    await checkVerbs()
    await checkEscapes()
    await checkCanvas()
    summary.textContent = failed === 0 ? 'All ' + passed + ' checks pass.' : failed + ' FAILED, ' + passed + ' passed.'
    summary.className = failed === 0 ? 'pass' : 'fail'
  })
})()
