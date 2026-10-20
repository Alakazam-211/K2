// @ts-check
// K2's frame runtime: the `k2` object a custom Garden widget talks to K2
// through (prd-zen-user-widgets-v2 UWA5, UW15, §7). Plain JavaScript, no
// dependencies. K2 inlines it as the first nonced script of every sealed
// widget frame, so `window.k2` exists before the widget's scripts run.
// contract-gen (UWA11) wraps it after the catalog's widget table
// (`K2_CONTRACT`) into `sdk/generated/k2-frame.js`; rerun it after an edit.
//
// Transport: the hello's MessagePort. The frame never holds a token.
//   frame → host {id, verb, args} · {sub, verb, args} · {unsub} · {ready}
//                {pong} · {error: {message, stack?}} · {chord}
//                {pointer: 'down' | 'up'} (trusted primary-button presses)
//   host → frame {id, ok, value} · {id, ok: false, error} · {sub, value}
//                {sub, error} · {ping}
// The hello comes on the frame's load, AFTER the widget's top-level script
// runs. Calls made before it wait; `k2.can`, `k2.config`, `k2.widget` and
// `k2.motion` don't, so a widget awaits `k2.connected` (resolves with the
// hello) before reading them. With no hello in 10 s, waiting calls and
// `k2.connected` reject with K2Error `failed` "not connected". A refused
// subscription gets `{sub, error}` once and ends: its onError (after cb)
// runs or, with none, K2 hears of it.
//
// 0.46.1 (catalog 11): K2Error carries `retryable` (the catalog's flag). A
// subscribe helper given a handler object `{data, error?, state?}` instead of
// `cb, onError` re-subscribes on its own after a retryable refusal (1, 2, 4,
// 8, 16, then every 30 s), runs the value through a shape guard first (a
// sources view is normalised; a bad one reaches `error` as bad_shape), and
// calls nothing after its unsubscribe. Arguments that aren't plain data are
// refused in the frame as bad_args. `k2.shape` holds the guards.
;(function () {
  'use strict'

  /** @typedef {{cap: string | null, reach: 'portable' | 'local', kind: 'call' | 'subscribe' | 'event', feature?: string}} VerbRow */
  /** @typedef {{code: string, message: string, cap?: string, room?: string, feature?: string, last?: unknown}} WireError */

  var g = /** @type {any} */ (self)
  /** @type {Record<string, VerbRow>} */
  // eslint-disable-next-line no-undef
  var TABLE = typeof K2_CONTRACT !== 'undefined' && K2_CONTRACT && K2_CONTRACT.verbs ? K2_CONTRACT.verbs : {}
  /** @type {string[]} */
  // eslint-disable-next-line no-undef
  var RETRY = typeof K2_CONTRACT !== 'undefined' && K2_CONTRACT && K2_CONTRACT.retryable ? K2_CONTRACT.retryable : []
  if (g.k2) return

  class K2Error extends Error {
    /**
     * @param {WireError} wire
     * @param {string} verb
     */
    constructor(wire, verb) {
      super(wire.message || wire.code)
      this.name = 'K2Error'
      this.code = wire.code
      this.verb = verb
      this.retryable = RETRY.indexOf(wire.code) >= 0
      if (wire.cap !== undefined) this.cap = wire.cap
      if (wire.room !== undefined) this.room = wire.room
      if (wire.feature !== undefined) this.feature = wire.feature
      // Garden v3: `unreachable` carries the last view K2 has (phase stale).
      if (wire.last !== undefined) this.last = wire.last
    }
  }

  /** @type {MessagePort | null} */
  var port = null
  /** @type {{caps: string[], features: string[], widget: {id: string, name: string, garden: string}, config: Record<string, unknown>, motion: {reduced: boolean}} | null} */
  var hello = null
  var nextId = 1
  /** @type {Map<number, {resolve: (v: unknown) => void, reject: (e: unknown) => void, verb: string}>} */
  var pending = new Map()
  /** @typedef {{cb: (v: unknown) => void, onError: ((e: K2Error) => void) | null, verb: string}} Sub */
  /** @type {Map<number, Sub>} */
  var subs = new Map()

  // Messages sent before the hello wait, in order; with none in CONNECT_MS
  // they fail "not connected" (K2 shows "didn't start").
  var CONNECT_MS = 10000
  /** @type {unknown[] | null} */
  var early = []
  /** @type {(v: unknown) => void} */
  var helloed = function () {}
  /** @type {(e: unknown) => void} */
  var unhelloed = helloed
  var connected = new Promise(function (res, rej) {
    helloed = res
    unhelloed = rej
  })
  connected.catch(function () {})

  /** @param {unknown} msg */
  function send(msg) {
    if (port) port.postMessage(msg)
    else if (early && early.length < 1000) early.push(msg)
  }

  // Plain data only: what a JSON round trip keeps (and undefined). Depth
  // caps cycles.
  /** @param {unknown} v @param {number} d @returns {boolean} */
  function plain(v, d) {
    if (d > 64) return false
    if (v === null || v === undefined || typeof v === 'string' || typeof v === 'boolean') return true
    if (typeof v === 'number') return isFinite(v)
    if (typeof v !== 'object') return false
    if (Array.isArray(v)) {
      for (var i = 0; i < v.length; i++) if (!plain(v[i], d + 1)) return false
      return true
    }
    // A plain object from any realm: its prototype is an Object.prototype.
    var proto = Object.getPrototypeOf(v)
    if (proto !== null && Object.getPrototypeOf(proto) !== null) return false
    for (var k in v) if (Object.prototype.hasOwnProperty.call(v, k) && !plain(/** @type {any} */ (v)[k], d + 1)) return false
    return true
  }

  /** @param {string} verb */
  function badArgs(verb) {
    return new K2Error({ code: 'bad_args', message: verb + ': arguments are plain data only (no functions, DOM nodes, class instances or cycles)' }, verb)
  }

  /** @param {string} verb */
  function notConnected(verb) {
    return new K2Error({ code: 'failed', message: 'not connected' }, verb)
  }

  g.setTimeout(function () {
    if (port) return
    early = null
    unhelloed(notConnected('connected'))
    pending.forEach(function (p) {
      p.reject(notConnected(p.verb))
    })
    pending.clear()
    var waiting = Array.from(subs.values())
    subs.clear()
    waiting.forEach(function (s) {
      refused(s, notConnected(s.verb))
    })
  }, CONNECT_MS)

  /** @param {any} r */
  function report(r) {
    send({ error: { message: String(r && r.message ? r.message : r), stack: r && r.stack ? String(r.stack).slice(0, 2000) : undefined } })
  }

  /**
   * @param {Sub} s
   * @param {K2Error} err
   */
  function refused(s, err) {
    if (!s.onError) return send({ error: { message: 'k2: ' + s.verb + ' refused (' + err.code + '): ' + err.message } })
    try {
      s.onError(err)
    } catch (e) {
      report(e)
    }
  }

  /**
   * One adapter: the hello's MessagePort.
   * @type {{call(verb: string, args: unknown[]): Promise<unknown>, subscribe(verb: string, args: unknown[], cb: (v: unknown) => void, onError: ((e: K2Error) => void) | null): () => void, info(): {caps: string[], features: string[], mode: 'frame'}}}
   */
  var transport = {
    call: function (verb, args) {
      if (!plain(args, 0)) return Promise.reject(badArgs(verb))
      if (!port && !early) return Promise.reject(notConnected(verb))
      var id = nextId++
      return new Promise(function (resolve, reject) {
        pending.set(id, { resolve: resolve, reject: reject, verb: verb })
        send({ id: id, verb: verb, args: args })
      })
    },
    subscribe: function (verb, args, cb, onError) {
      if (typeof cb !== 'function') throw new TypeError('k2.subscribe: the callback must be a function')
      if (!plain(args, 0)) throw badArgs(verb)
      if (!port && !early) throw notConnected(verb)
      var sub = nextId++
      subs.set(sub, { cb: cb, onError: onError, verb: verb })
      send({ sub: sub, verb: verb, args: args })
      var live = true
      return function unsubscribe() {
        if (!live) return
        live = false
        subs.delete(sub)
        send({ unsub: sub })
      }
    },
    info: function () {
      return { caps: hello ? hello.caps.slice() : [], features: hello ? hello.features.slice() : [], mode: 'frame' }
    },
  }

  /** @param {MessageEvent} e */
  function onPortMessage(e) {
    var m = e.data
    if (!m || typeof m !== 'object') return
    if (typeof m.ping === 'number') {
      send({ pong: m.ping })
      return
    }
    if (typeof m.sub === 'number' && 'value' in m) {
      var s = subs.get(m.sub)
      if (s) s.cb(m.value)
      return
    }
    if (typeof m.sub === 'number') {
      var gone = subs.get(m.sub)
      if (!gone) return
      subs.delete(m.sub)
      refused(gone, new K2Error(m.error || { code: 'failed', message: 'refused' }, gone.verb))
      return
    }
    if (typeof m.id === 'number') {
      var p = pending.get(m.id)
      if (!p) return
      pending.delete(m.id)
      if (m.ok === true) p.resolve(m.value)
      else p.reject(new K2Error(m.error || { code: 'failed', message: 'refused' }, p.verb))
    }
  }

  /** The pushed theme's `--zen-*` vars and scheme onto :root (UW39). */
  /** @param {unknown} v */
  function applyTheme(v) {
    if (!v || typeof v !== 'object') return
    var info = /** @type {{theme?: {vars?: Record<string, string>, scheme?: string}}} */ (v)
    var t = info.theme
    if (!t || typeof t !== 'object') return
    var root = document.documentElement
    if (t.vars) {
      for (var k in t.vars) {
        if (k.indexOf('--zen-') === 0 && typeof t.vars[k] === 'string') root.style.setProperty(k, t.vars[k])
      }
    }
    if (t.scheme === 'light' || t.scheme === 'dark') {
      root.setAttribute('data-zen-scheme', t.scheme)
      // The Garden's scheme wins over the widget's own (0.46.3), as the
      // frame's first style does: a switch keeps it in step.
      root.style.setProperty('color-scheme', t.scheme, 'important')
    }
  }

  /** @param {MessageEvent} e */
  function onHello(e) {
    if (port) return
    if (e.source !== g.parent) return
    var m = e.data
    if (!m || m.k2 !== 'hello' || m.v !== 1 || !e.ports || !e.ports[0]) return
    port = e.ports[0]
    hello = {
      caps: Array.isArray(m.caps) ? m.caps.slice() : [],
      features: Array.isArray(m.features) ? m.features.slice() : [],
      widget: m.widget && typeof m.widget === 'object' ? Object.freeze(Object.assign({}, m.widget)) : { id: '', name: '', garden: '' },
      config: Object.freeze(Object.assign({}, m.config || {})),
      motion: Object.freeze({ reduced: !!(m.motion && m.motion.reduced) }),
    }
    port.onmessage = onPortMessage
    g.removeEventListener('message', onHello)
    // K2's own subscription: the frame follows the Garden's theme.
    transport.subscribe('theme.changed', [], applyTheme, null)
    var waiting = early || []
    early = null
    for (var i = 0; i < waiting.length; i++) port.postMessage(waiting[i])
    helloed(Object.freeze({ caps: hello.caps.slice(), features: hello.features.slice(), widget: hello.widget, config: hello.config, motion: hello.motion }))
  }
  g.addEventListener('message', onHello)

  // ── Errors and keys reported to K2 (UW30, UW33) ─────────────────────────

  g.addEventListener('error', function (/** @type {ErrorEvent} */ e) {
    report({ message: e.message || 'error', stack: e.error && e.error.stack })
  })
  g.addEventListener('unhandledrejection', function (/** @type {PromiseRejectionEvent} */ e) {
    report(e.reason)
  })

  var MAC = /Mac/.test(String(g.navigator && g.navigator.platform))
  // Mirrors `zenForwardedChordForKey` (lib/zen/zen-shortcut.ts); the host
  // gates each one again.
  /** @param {KeyboardEvent} e */
  function chordOf(e) {
    if (e.isTrusted === false || e.repeat) return null
    var altGr = typeof e.getModifierState === 'function' && e.getModifierState('AltGraph')
    if (!MAC && e.code === 'KeyZ' && e.ctrlKey && e.altKey && !e.metaKey && !e.shiftKey && !altGr) return 'zen-exit'
    var digit = /^Digit([1-9])$/.exec(e.code)
    if (digit && e.metaKey && e.altKey && !e.ctrlKey && !e.shiftKey) return 'garden-' + digit[1]
    if (e.code === 'Period') {
      var mods = MAC ? e.ctrlKey && e.metaKey && !e.altKey : e.ctrlKey && e.altKey && !e.metaKey && !altGr
      if (mods) return e.shiftKey ? 'theme-prev' : 'theme-next'
    }
    return null
  }
  g.addEventListener(
    'keydown',
    function (/** @type {KeyboardEvent} */ e) {
      var chord = chordOf(e)
      if (!chord) return
      e.preventDefault()
      send({ chord: chord })
    },
    true,
  )

  // ── The whole canvas (window.startDrag, canvas.setHitRegions) ───────────

  // A trusted primary-button press in this frame, told to K2 as it happens
  // (capture phase, before the widget's own handlers): K2 honours
  // window.startDrag only while one is down. One drag per press.
  var down = false
  g.addEventListener(
    'pointerdown',
    function (/** @type {PointerEvent} */ e) {
      if (e.isTrusted === false || e.button !== 0) return
      down = true
      send({ pointer: 'down' })
    },
    true,
  )
  function up() {
    if (!down) return
    down = false
    send({ pointer: 'up' })
  }
  g.addEventListener('pointerup', up, true)
  g.addEventListener('pointercancel', up, true)
  g.addEventListener('blur', up)

  function startDrag() {
    if (!down) return Promise.reject(new K2Error({ code: 'failed', message: 'window.startDrag needs a mouse button down in this widget' }, 'window.startDrag'))
    down = false
    return transport.call('window.startDrag', [])
  }

  // Hit regions are coalesced: at most one send every REGION_MS, carrying
  // the latest list, and an unchanged list isn't sent again. A widget may
  // call it on every animation frame.
  var REGION_MS = 100
  var regionsAt = 0
  var regionsSent = ''
  /** @type {{list: unknown, key: string, waiters: Array<[(v: unknown) => void, (e: unknown) => void]>} | null} */
  var regionsQ = null
  /** @param {unknown} rects */
  function setHitRegions(rects) {
    var list = Array.isArray(rects)
      ? rects.map(function (r) {
          return r && typeof r === 'object' ? { x: r.x, y: r.y, width: r.width, height: r.height } : r
        })
      : rects
    var key = JSON.stringify(list === undefined ? null : list)
    if (!regionsQ && key === regionsSent) return Promise.resolve(null)
    if (!regionsQ) {
      regionsQ = { list: list, key: key, waiters: [] }
      g.setTimeout(flushRegions, Math.max(0, regionsAt + REGION_MS - Date.now()))
    }
    var q = regionsQ
    q.list = list
    q.key = key
    return new Promise(function (res, rej) {
      q.waiters.push([res, rej])
    })
  }
  function flushRegions() {
    var q = regionsQ
    if (!q) return
    regionsQ = null
    regionsAt = Date.now()
    transport.call('canvas.setHitRegions', [q.list]).then(
      function (v) {
        regionsSent = q.key
        q.waiters.forEach(function (w) {
          w[0](v)
        })
      },
      function (e) {
        regionsSent = ''
        q.waiters.forEach(function (w) {
          w[1](e)
        })
      },
    )
  }

  // ── Garden data: transactions and chunked values ───────────────────────

  // One transaction part: at most 64 ops and 60 KiB of values (UTF-8).
  var PART_OPS = 64
  var PART_BYTES = 60 * 1024
  // A chunk's text, as JSON, stays under this (one part per chunk).
  var CHUNK_BYTES = 48 * 1024
  var CHUNKED_MAX = 1024 * 1024

  /** @param {string} verb @param {string} message */
  function fail(verb, message) {
    return new K2Error({ code: 'too_large', message: message }, verb)
  }

  /** UTF-8 bytes of a string. @param {string} s */
  function utf8Len(s) {
    var n = 0
    for (var i = 0; i < s.length; i++) {
      var c = s.charCodeAt(i)
      if (c < 0x80) n += 1
      else if (c < 0x800) n += 2
      else if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {
        n += 4
        i++
      } else n += 3
    }
    return n
  }

  /** UTF-8 bytes of a value as JSON (`undefined` counts as null). @param {unknown} v */
  function jsonBytes(v) {
    var s = JSON.stringify(v)
    return utf8Len(s === undefined ? 'null' : s)
  }

  /** Ops in parts of ≤ 64 ops and ≤ 60 KiB of values. @param {any[]} ops */
  function parts(ops) {
    /** @type {any[][]} */
    var out = []
    var cur = []
    var bytes = 0
    for (var i = 0; i < ops.length; i++) {
      var b = ops[i] && ops[i].op === 'put' ? jsonBytes(ops[i].value) : 0
      if (cur.length && (cur.length >= PART_OPS || bytes + b > PART_BYTES)) {
        out.push(cur)
        cur = []
        bytes = 0
      }
      cur.push(ops[i])
      bytes += b
    }
    if (cur.length) out.push(cur)
    return out
  }

  // k2.store.tx(): begins on the first add; parts go in order.
  function makeTx() {
    /** @type {Promise<string> | null} */
    var id = null
    /** @type {Promise<unknown>} */
    var q = Promise.resolve()
    function begin() {
      if (!id) id = call('store.txBegin', []).then(function (r) { return /** @type {any} */ (r).txId })
      return id
    }
    /** @param {() => Promise<unknown>} f */
    function next(f) {
      var p = q.then(f)
      q = p.catch(function () {})
      return p
    }
    return Object.freeze({
      add: function (/** @type {any[]} */ ops) {
        if (!Array.isArray(ops) || !ops.length) return Promise.reject(new K2Error({ code: 'failed', message: 'tx.add takes a list of ops' }, 'store.txAdd'))
        return next(function () {
          return begin().then(function (t) {
            /** @type {Promise<unknown>} */
            var p = Promise.resolve(null)
            parts(ops).forEach(function (part) {
              p = p.then(function () { return call('store.txAdd', [t, part]) })
            })
            return p
          })
        })
      },
      commit: function () {
        return next(function () {
          return begin().then(function (t) { return call('store.txCommit', [t]) })
        })
      },
      abort: function () {
        return next(function () {
          return id ? id.then(function (t) { return call('store.txAbort', [t]) }) : Promise.resolve({ aborted: false })
        })
      },
    })
  }

  /** @param {string} key */
  function partKey(key, i) {
    return key + '/~' + i
  }

  // putChunked(key, text, {ifRev}): parts <key>/~0… and the header <key>
  // {chunks, bytes, sha256}, plus deletes of old parts past the new end, in
  // one transaction.
  /** @param {string} key @param {string} text @param {{ifRev?: number}} [opts] */
  function putChunked(key, text, opts) {
    var verb = 'store.putChunked'
    if (typeof key !== 'string' || typeof text !== 'string') return Promise.reject(new K2Error({ code: 'failed', message: 'putChunked(key, text)' }, verb))
    var total = utf8Len(text)
    if (total > CHUNKED_MAX) return Promise.reject(fail(verb, 'putChunked holds at most 1 MiB of text'))
    /** @type {string[]} */
    var chunks = []
    var start = 0
    var size = 0
    for (var i = 0; i < text.length; i++) {
      var c = text.charCodeAt(i)
      var pair = c >= 0xd800 && c <= 0xdbff && i + 1 < text.length
      var b = c >= 0x20 && c < 0x7f && c !== 0x22 && c !== 0x5c ? 1 : jsonBytes(pair ? text.slice(i, i + 2) : text[i]) - 2
      if (size + b > CHUNK_BYTES) {
        chunks.push(text.slice(start, i))
        start = i
        size = 0
      }
      size += b
      if (pair) i++
    }
    if (start < text.length || !chunks.length) chunks.push(text.slice(start))
    var header
    try {
      header = { chunks: chunks.length, bytes: total, sha256: sha256Hex(text) }
    } catch (e) {
      return Promise.reject(new K2Error({ code: 'failed', message: 'putChunked takes well-formed text' }, verb))
    }
    /** @type {any[]} */
    var ops = chunks.map(function (t, n) {
      return { op: 'put', key: partKey(key, n), value: { part: n, text: t } }
    })
    var head = { op: 'put', key: key, value: header }
    if (opts && typeof opts.ifRev === 'number') /** @type {any} */ (head).ifRev = opts.ifRev
    ops.push(head)
    return call('store.list', [{ prefix: key + '/~', limit: 500 }]).then(function (page) {
      var items = /** @type {any} */ (page).items || []
      items.forEach(function (/** @type {any} */ e) {
        var n = Number(String(e.key).slice(key.length + 2))
        if (!(n < chunks.length)) ops.push({ op: 'delete', key: e.key })
      })
      var tx = makeTx()
      return tx.add(ops).then(function () { return tx.commit() }, function (err) {
        tx.abort().catch(function () {})
        throw err
      })
    })
  }

  /** @param {string} key */
  function getChunked(key) {
    var verb = 'store.getChunked'
    return call('store.get', [key]).then(function (h) {
      var head = /** @type {any} */ (h)
      if (!head) return null
      var v = head.value
      if (!v || typeof v.chunks !== 'number' || typeof v.sha256 !== 'string') throw mismatch(verb, key + ' isn’t a putChunked header')
      /** @type {Promise<unknown>[]} */
      var reads = []
      for (var n = 0; n < v.chunks; n++) reads.push(call('store.get', [partKey(key, n)]))
      return Promise.all(reads).then(function (got) {
        var text = got
          .map(function (e, n) {
            var p = /** @type {any} */ (e)
            if (!p || !p.value || p.value.part !== n || typeof p.value.text !== 'string') throw mismatch(verb, 'part ' + n + ' of ' + key + ' is missing or wrong')
            return p.value.text
          })
          .join('')
        if (utf8Len(text) !== v.bytes || sha256Hex(text) !== v.sha256) throw mismatch(verb, key + '’s parts don’t match its header')
        return text
      })
    })
  }

  /** @param {string} verb @param {string} message */
  function mismatch(verb, message) {
    var e = /** @type {any} */ (new K2Error({ code: 'failed', message: message }, verb))
    e.reason = 'chunk_mismatch'
    return e
  }

  /** SHA-256 of a string's UTF-8 bytes, hex. @param {string} s */
  function sha256Hex(s) {
    var bytes = unescape(encodeURIComponent(s))
    var K = [], H = [1779033703, 3144134277, 1013904242, 2773480762, 1359893119, 2600822924, 528734635, 1541459225]
    for (var p = 2, found = 0; found < 64; p++) {
      for (var d = 2; d * d <= p && p % d; d++);
      if (d * d > p) K[found++] = (Math.pow(p, 1 / 3) * 4294967296) | 0
    }
    var l = bytes.length
    var words = []
    for (var i = 0; i < l; i++) words[i >> 2] |= bytes.charCodeAt(i) << (24 - (i % 4) * 8)
    words[l >> 2] |= 0x80 << (24 - (l % 4) * 8)
    words[(((l + 8) >> 6) << 4) + 15] = l * 8
    for (var j = 0; j < words.length; j += 16) {
      var w = [], h = H.slice(0)
      for (var t = 0; t < 64; t++) {
        var x = w[t - 15], y = w[t - 2]
        w[t] = t < 16 ? words[j + t] | 0 : (w[t - 16] + ((x >>> 7 | x << 25) ^ (x >>> 18 | x << 14) ^ (x >>> 3)) + w[t - 7] + ((y >>> 17 | y << 15) ^ (y >>> 19 | y << 13) ^ (y >>> 10))) | 0
        var a = h[0], e = h[4]
        var t1 = h[7] + ((e >>> 6 | e << 26) ^ (e >>> 11 | e << 21) ^ (e >>> 25 | e << 7)) + ((e & h[5]) ^ (~e & h[6])) + K[t] + w[t]
        var t2 = ((a >>> 2 | a << 30) ^ (a >>> 13 | a << 19) ^ (a >>> 22 | a << 10)) + ((a & h[1]) ^ (a & h[2]) ^ (h[1] & h[2]))
        h = [(t1 + t2) | 0].concat(h)
        h[4] = (h[4] + t1) | 0
        h.length = 8
      }
      for (var k = 0; k < 8; k++) H[k] = (H[k] + h[k]) | 0
    }
    return H.map(function (v) { return ('00000000' + (v >>> 0).toString(16)).slice(-8) }).join('')
  }

  // ── The k2 object ───────────────────────────────────────────────────────

  /**
   * @param {string} verb
   * @param {unknown[]} args
   */
  function call(verb, args) {
    if (verb === 'canvas.setHitRegions') return setHitRegions(args[0])
    if (verb === 'window.startDrag') return startDrag()
    return transport.call(verb, args)
  }

  // ── Guards for data the widget didn't make (k2.shape) ───────────────────

  var HEALTH = ['ok', 'warn', 'crit', 'unknown']
  var PHASE = ['pending', 'live', 'stale', 'paused']
  /** @param {unknown} v @returns {v is Record<string, any>} */
  function isObj(v) {
    return !!v && typeof v === 'object' && !Array.isArray(v)
  }
  /** @param {unknown} v */
  function obj(v) {
    return isObj(v) ? v : {}
  }
  /** @param {unknown} v @returns {unknown[]} */
  function list(v) {
    return Array.isArray(v) ? v : []
  }
  /** @param {unknown} v @param {number} [f] */
  function num(v, f) {
    return typeof v === 'number' && isFinite(v) ? v : f === undefined ? 0 : f
  }
  /** @param {unknown} v @param {string} [f] @param {number} [max] */
  function str(v, f, max) {
    var t = typeof v === 'string' ? v : f === undefined ? '' : f
    return max !== undefined && t.length > max ? t.slice(0, max) : t
  }
  /** @param {unknown} v @param {string[]} set @param {string} f */
  function oneOf(v, set, f) {
    return typeof v === 'string' && set.indexOf(v) >= 0 ? v : f
  }
  /** @param {unknown} v */
  function strOrNull(v) {
    return typeof v === 'string' ? v : null
  }
  /** @param {unknown} v */
  function numOrNull(v) {
    return typeof v === 'number' && isFinite(v) ? v : null
  }
  /** @param {unknown} v */
  function soft(v) {
    var o = obj(v)
    return { count: num(o.count), of: num(o.of, 1), level: strOrNull(o.level) }
  }
  // A sources card view with every key a card draws from; null when `v`
  // isn't one. Unknown keys are kept.
  /** @param {unknown} v */
  function view(v) {
    if (!isObj(v) || (typeof v.display !== 'string' && typeof v.health !== 'string')) return null
    var out = Object.assign({}, v)
    var health = oneOf(v.health, HEALTH, 'unknown')
    out.name = str(v.name)
    out.label = str(v.label, out.name)
    out.health = health
    out.phase = oneOf(v.phase, PHASE, 'pending')
    out.display = str(v.display, health)
    out.reason = strOrNull(v.reason)
    out.reasonText = strOrNull(v.reasonText)
    out.fields = obj(v.fields)
    out.items = list(v.items)
      .filter(isObj)
      .map(function (/** @type {any} */ it) {
        var i = Object.assign({}, it)
        i.key = typeof it.key === 'string' || typeof it.key === 'number' ? String(it.key) : ''
        i.health = oneOf(it.health, HEALTH, 'unknown')
        i.reason = strOrNull(it.reason)
        i.fields = obj(it.fields)
        i.soft = soft(it.soft)
        return i
      })
    var c = obj(v.counts)
    out.counts = { ok: num(c.ok), warn: num(c.warn), crit: num(c.crit), unknown: num(c.unknown) }
    out.soft = soft(v.soft)
    out.serverNow = num(v.serverNow, Math.floor(Date.now() / 1000))
    out.updatedAt = numOrNull(v.updatedAt)
    return out
  }
  /** A store delivery: changed keys, or a resync list. @param {unknown} v */
  function delivery(v) {
    if (Array.isArray(v)) return v.filter(isObj)
    if (isObj(v) && v.resync === true && Array.isArray(v.items)) return v
    return null
  }
  /** @type {Record<string, (v: unknown) => unknown>} */
  var SHAPES = { 'sources.subscribe': view, 'store.changes': delivery }

  // A handler-object subscription: retries retryable refusals with capped
  // backoff; nothing runs after its unsubscribe. `legacy` (0.46.3): a
  // `sources.subscribe(address, cb, onError?)` gets the same retries, with
  // its raw values (no shape guard) and never on rate_limited. With no error
  // handler K2 hears of a refusal once per run of refusals, not per retry.
  /** @param {string} verb @param {unknown[]} args @param {{data: (v: unknown) => void, error?: ((e: K2Error) => void) | null, state?: (s: string) => void, legacy?: boolean}} h */
  function watch(verb, args, h) {
    var live = true
    var attempt = 0
    var now = ''
    /** @type {(() => void) | null} */
    var off = null
    /** @type {unknown} */
    var timer = null
    var guard = h.legacy ? null : SHAPES[verb]
    /** @param {string} st */
    function state(st) {
      if (!live || st === now) return
      now = st
      if (h.state) {
        try {
          h.state(st)
        } catch (e) {
          report(e)
        }
      }
    }
    /** @param {K2Error} err */
    function fault(err) {
      if (!live) return
      if (!h.error) {
        if (/** @type {any} */ (err).retrying && attempt > 1) return
        return send({ error: { message: 'k2: ' + verb + ' (' + err.code + '): ' + err.message } })
      }
      try {
        h.error(err)
      } catch (e) {
        report(e)
      }
    }
    /** @param {unknown} v */
    function data(v) {
      if (!live) return
      var safe = guard ? guard(v) : v
      if (guard && safe === null) {
        var bad = /** @type {any} */ (new K2Error({ code: 'bad_shape', message: verb + ' delivered a value that isn’t its shape; keep your last good data' }, verb))
        bad.retrying = true
        bad.attempt = attempt
        return fault(bad)
      }
      attempt = 0
      state('live')
      try {
        h.data(safe)
      } catch (e) {
        report(e)
      }
    }
    /** @param {K2Error} err */
    function refusal(err) {
      if (!live) return
      off = null
      attempt++
      var e = /** @type {any} */ (err)
      e.attempt = attempt
      if (err.retryable && !(h.legacy && err.code === 'rate_limited')) {
        if (e.last !== undefined && guard) e.last = guard(e.last)
        e.retrying = true
        // The old form is retrying before its onError runs, so an onError
        // that subscribes again replaces this retry (no pile-up).
        if (!h.legacy) fault(err)
        state('retrying')
        timer = g.setTimeout(start, Math.min(30000, 1000 * Math.pow(2, Math.min(attempt - 1, 5))))
        if (h.legacy) fault(err)
        return
      }
      e.retrying = false
      fault(err)
      state('ended')
      live = false
    }
    function start() {
      timer = null
      if (!live) return
      try {
        off = transport.subscribe(verb, args, data, refusal)
      } catch (e) {
        refusal(e instanceof K2Error ? e : new K2Error({ code: 'failed', message: String(e && /** @type {any} */ (e).message ? /** @type {any} */ (e).message : e) }, verb))
      }
    }
    start()
    return function unsubscribe() {
      if (!live) return
      live = false
      if (timer !== null && g.clearTimeout) g.clearTimeout(timer)
      timer = null
      if (off) off()
      off = null
    }
  }

  // Callback-form source subscriptions K2 is retrying, by verb + args. A new
  // one to the same source replaces them, so a widget that re-subscribes in
  // its own onError (as cards did before 0.46.3) never piles up retries.
  /** @type {Map<string, Array<{st: string, stop: () => void}>>} */
  var retried = new Map()

  // `sources.subscribe(address, cb, onError?)` (0.46.3): a retryable refusal
  // (the placement doesn't bind it yet, its server is away) reaches onError
  // with `retrying` and K2 subscribes again by itself, like a handler object.
  /** @param {string} verb @param {unknown[]} args @param {(v: unknown) => void} cb @param {((e: K2Error) => void) | null} onError */
  function legacyWatch(verb, args, cb, onError) {
    if (typeof cb !== 'function') throw new TypeError('k2.subscribe: the callback must be a function')
    if (!plain(args, 0)) throw badArgs(verb)
    if (!port && !early) throw notConnected(verb)
    var key = verb + '\n' + JSON.stringify(args)
    var list = (retried.get(key) || []).filter(function (w) {
      if (w.st !== 'retrying') return true
      w.stop()
      return false
    })
    retried.set(key, list)
    var w = { st: '', stop: function () {} }
    var off = watch(verb, args, {
      data: cb,
      error: onError,
      legacy: true,
      state: function (st) {
        w.st = st
      },
    })
    w.stop = function () {
      var now = retried.get(key) || []
      var i = now.indexOf(w)
      if (i >= 0) now.splice(i, 1)
      if (!now.length) retried.delete(key)
      off()
    }
    list.push(w)
    return w.stop
  }

  // Trailing function args are cb, onError?; or one handler object.
  /** @param {string} verb @param {unknown[]} rest */
  function subscribe(verb, rest) {
    var last = /** @type {any} */ (rest[rest.length - 1])
    if (isObj(last) && typeof last.data === 'function') return watch(verb, rest.slice(0, -1), last)
    var n = rest.length
    var two = n >= 2 && typeof rest[n - 1] === 'function' && typeof rest[n - 2] === 'function'
    var cb = two ? rest[n - 2] : rest[n - 1]
    var onError = two ? /** @type {(e: K2Error) => void} */ (rest[n - 1]) : null
    var args = rest.slice(0, two ? -2 : -1)
    if (verb === 'sources.subscribe') return legacyWatch(verb, args, /** @type {(v: unknown) => void} */ (cb), onError)
    return transport.subscribe(verb, args, /** @type {(v: unknown) => void} */ (cb), onError)
  }

  /** @type {Record<string, any>} */
  var k2 = {}
  k2.call = function (/** @type {string} */ verb) {
    return call(verb, Array.prototype.slice.call(arguments, 1))
  }
  k2.subscribe = function (/** @type {string} */ verb) {
    return subscribe(verb, Array.prototype.slice.call(arguments, 1))
  }
  k2.on = k2.subscribe
  k2.can = function (/** @type {string} */ verb) {
    var row = TABLE[verb]
    if (!row || !hello) return false
    if (row.cap !== null && hello.caps.indexOf(row.cap) < 0) return false
    if (row.feature && hello.features.indexOf(row.feature) < 0) return false
    return true
  }
  k2.connected = connected
  k2.ready = function () {
    send({ ready: true })
  }
  k2.asset = function (/** @type {string} */ name) {
    var assets = g.K2_ASSETS
    return assets && typeof assets[name] === 'string' ? assets[name] : null
  }
  Object.defineProperty(k2, 'config', { enumerable: true, get: function () { return hello ? hello.config : Object.freeze({}) } })
  // `k2.widget`: the hello's id, name and garden, plus the catalog's
  // `widget.*` helpers (`widget.self`, room-scope R16) on the same object.
  // Before the hello it holds the helpers only (null when there are none).
  /** @type {unknown} */
  var widgetFor = null
  /** @type {Record<string, unknown> | null} */
  var widgetView = null
  /** @type {Record<string, unknown> | null} */
  var widgetHelpers = null
  Object.defineProperty(k2, 'widget', {
    enumerable: true,
    get: function () {
      if (!hello) {
        if (!groups.widget) return null
        return widgetHelpers || (widgetHelpers = Object.freeze(Object.assign({}, groups.widget)))
      }
      if (widgetFor !== hello) {
        widgetFor = hello
        widgetView = Object.freeze(Object.assign({}, groups.widget || {}, hello.widget))
      }
      return widgetView
    },
  })
  Object.defineProperty(k2, 'motion', { enumerable: true, get: function () { return hello ? hello.motion : Object.freeze({ reduced: false }) } })
  k2.shape = Object.freeze({ view: view, list: list, obj: obj, num: num, str: str })
  k2.K2Error = K2Error
  k2.retryable = Object.freeze(RETRY.slice())

  // Named helpers, one per row (`surface.action` → k2.surface.action).
  /** @type {Record<string, Record<string, Function>>} */
  var groups = {}
  Object.keys(TABLE).forEach(function (verb) {
    var dot = verb.indexOf('.')
    if (dot <= 0) return
    var surface = verb.slice(0, dot)
    var action = verb.slice(dot + 1)
    if (surface in k2 && !(surface in groups) && surface !== 'widget') return
    var row = TABLE[verb]
    var group = groups[surface] || (groups[surface] = {})
    group[action] =
      row.kind === 'call'
        ? function () {
            return call(verb, Array.prototype.slice.call(arguments))
          }
        : function () {
            return subscribe(verb, Array.prototype.slice.call(arguments))
          }
  })
  // ── Garden data helpers (contract-garden-data-v1 §7.2, §7.4) ────────────

  if (groups.store) {
    var store = groups.store
    // k2.store.subscribe(prefix | {prefix, since, from}, cb, onError?): whole revs
    // from any writer; K2 pages store.changes on each change. `from` (0.46.1)
    // follows a neighbour widget named in the manifest's reads.
    store.subscribe = function () {
      return subscribe('store.changes', Array.prototype.slice.call(arguments))
    }
    store.tx = makeTx
    store.putChunked = putChunked
    store.getChunked = getChunked
  }
  if (groups.viewer) {
    var viewerGet = groups.viewer.get
    var viewer = function () {
      return viewerGet()
    }
    groups.viewer = Object.assign(viewer, groups.viewer)
  }

  Object.keys(groups).forEach(function (surface) {
    if (surface === 'widget') return // served by the k2.widget getter
    k2[surface] = Object.freeze(groups[surface])
  })

  Object.defineProperty(g, 'k2', { value: Object.freeze(k2), writable: false, configurable: false, enumerable: true })
})()
