// @ts-check
// K2's frame runtime: the `k2` object a custom Garden widget talks to K2
// through (prd-zen-user-widgets-v2 UWA5, UW15, §7). Plain JavaScript, no
// dependencies, no compile step: K2 inlines it as the first nonced script
// of every sealed widget frame, so `window.k2` exists before the widget's
// own scripts run, and the widget can't load or replace it.
//
// Delivery: `sdk/generated/k2-frame.js` (B1's contract-gen, UWA11) is
//   self.__K2_VERBS__ = { "<verb>": {cap, reach, kind, feature?}, ... };
// followed by this file. Until the generator lands, the renderer joins the
// same two parts itself (`lib/zen/zen-custom-prelude.ts`).
//
// Transport interface: {call(verb, args), subscribe(verb, args, cb), info()}.
// v2 wires one adapter, MessagePort, created from K2's hello; the cookie and
// server adapters are Cut B and are not written here. The frame never holds
// a token: no mode of this runtime takes one.
//
// Protocol (host = K2's renderer, over the hello's private port):
//   frame → host {id, verb, args} · {sub, verb, args} · {unsub} · {ready}
//                {pong} · {error: {message, stack?}} · {chord}
//   host → frame {id, ok, value} · {id, ok: false, error} · {sub, value}
//                {ping}
// Calls made before the hello wait for it (it comes on the frame's load);
// with no hello in 10 s they reject with K2Error `failed` "not connected".
(function () {
  'use strict'

  /** @typedef {{cap: string | null, reach: 'portable' | 'local', kind: 'call' | 'subscribe' | 'event', feature?: string}} VerbRow */
  /** @typedef {{code: string, message: string, cap?: string, room?: string, feature?: string}} WireError */

  var g = /** @type {any} */ (self)
  /** @type {Record<string, VerbRow>} */
  var TABLE = g.__K2_VERBS__ || {}
  try {
    delete g.__K2_VERBS__
  } catch (_e) {
    g.__K2_VERBS__ = undefined
  }
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
      if (wire.cap !== undefined) this.cap = wire.cap
      if (wire.room !== undefined) this.room = wire.room
      if (wire.feature !== undefined) this.feature = wire.feature
    }
  }

  /** @type {MessagePort | null} */
  var port = null
  /** @type {{caps: string[], features: string[], widget: {id: string, name: string, garden: string}, config: Record<string, unknown>, motion: {reduced: boolean}} | null} */
  var hello = null
  var nextId = 1
  /** @type {Map<number, {resolve: (v: unknown) => void, reject: (e: unknown) => void, verb: string}>} */
  var pending = new Map()
  /** @type {Map<number, (v: unknown) => void>} */
  var subs = new Map()

  // K2's hello arrives on the frame's `load`, after the widget's top-level
  // scripts ran. Messages sent before it wait (in order) and go out with
  // the hello; with no hello within CONNECT_MS, waiting calls reject with
  // K2Error `failed` "not connected" (and K2 shows "didn't start").
  var CONNECT_MS = 10000
  /** @type {unknown[] | null} */
  var early = []

  /** @param {unknown} msg */
  function send(msg) {
    if (port) port.postMessage(msg)
    else if (early && early.length < 1000) early.push(msg)
  }

  /** @param {string} verb */
  function notConnected(verb) {
    return new K2Error({ code: 'failed', message: 'not connected' }, verb)
  }

  g.setTimeout(function () {
    if (port) return
    early = null
    pending.forEach(function (p) {
      p.reject(notConnected(p.verb))
    })
    pending.clear()
    subs.clear()
  }, CONNECT_MS)

  /**
   * One adapter: the hello's MessagePort.
   * @type {{call(verb: string, args: unknown[]): Promise<unknown>, subscribe(verb: string, args: unknown[], cb: (v: unknown) => void): () => void, info(): {caps: string[], features: string[], mode: 'frame'}}}
   */
  var transport = {
    call: function (verb, args) {
      if (!port && !early) return Promise.reject(notConnected(verb))
      var id = nextId++
      return new Promise(function (resolve, reject) {
        pending.set(id, { resolve: resolve, reject: reject, verb: verb })
        send({ id: id, verb: verb, args: args })
      })
    },
    subscribe: function (verb, args, cb) {
      if (typeof cb !== 'function') throw new TypeError('k2.subscribe: the last argument must be a function')
      if (!port && !early) throw notConnected(verb)
      var sub = nextId++
      subs.set(sub, cb)
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
      var cb = subs.get(m.sub)
      if (cb) cb(m.value)
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

  /** The theme the host pushes: `--zen-*` variables and the scheme (UW39). */
  /** @param {unknown} v */
  function applyTheme(v) {
    if (!v || typeof v !== 'object') return
    var t = /** @type {{vars?: Record<string, string>, scheme?: string}} */ (v)
    var root = document.documentElement
    if (t.vars) {
      for (var k in t.vars) {
        if (k.indexOf('--zen-') === 0 && typeof t.vars[k] === 'string') root.style.setProperty(k, t.vars[k])
      }
    }
    if (t.scheme === 'light' || t.scheme === 'dark') root.setAttribute('data-zen-scheme', t.scheme)
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
    transport.subscribe('theme.changed', [], applyTheme)
    var waiting = early || []
    early = null
    for (var i = 0; i < waiting.length; i++) port.postMessage(waiting[i])
  }
  g.addEventListener('message', onHello)

  // ── Errors and keys reported to K2 (UW30, UW33) ─────────────────────────

  g.addEventListener('error', function (/** @type {ErrorEvent} */ e) {
    send({ error: { message: String(e.message || 'error'), stack: e.error && e.error.stack ? String(e.error.stack).slice(0, 2000) : undefined } })
  })
  g.addEventListener('unhandledrejection', function (/** @type {PromiseRejectionEvent} */ e) {
    var r = e.reason
    send({ error: { message: String(r && r.message ? r.message : r), stack: r && r.stack ? String(r.stack).slice(0, 2000) : undefined } })
  })

  var MAC = /Mac/.test(String(g.navigator && g.navigator.platform))
  /** @param {KeyboardEvent} e */
  function chordOf(e) {
    var digit = /^Digit([1-9])$/.exec(e.code)
    if (digit && e.altKey && (e.metaKey || (!MAC && e.ctrlKey))) return 'garden-' + digit[1]
    var themeMods = MAC ? e.ctrlKey && e.metaKey && !e.altKey : e.ctrlKey && e.altKey && !e.metaKey
    if (e.code === 'Period' && themeMods) return e.shiftKey ? 'theme-prev' : 'theme-next'
    if (!MAC && e.code === 'KeyZ' && e.ctrlKey && e.altKey && !e.metaKey && !(e.getModifierState && e.getModifierState('AltGraph'))) return 'zen-exit'
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

  // ── The k2 object ───────────────────────────────────────────────────────

  /**
   * @param {string} verb
   * @param {unknown[]} args
   */
  function call(verb, args) {
    return transport.call(verb, args)
  }

  /**
   * @param {string} verb
   * @param {unknown[]} rest  args…, cb
   */
  function subscribe(verb, rest) {
    var cb = rest[rest.length - 1]
    return transport.subscribe(verb, rest.slice(0, -1), /** @type {(v: unknown) => void} */ (cb))
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
  k2.ready = function () {
    send({ ready: true })
  }
  k2.asset = function (/** @type {string} */ name) {
    var assets = g.K2_ASSETS
    return assets && typeof assets[name] === 'string' ? assets[name] : null
  }
  Object.defineProperty(k2, 'config', { enumerable: true, get: function () { return hello ? hello.config : Object.freeze({}) } })
  Object.defineProperty(k2, 'widget', { enumerable: true, get: function () { return hello ? hello.widget : null } })
  Object.defineProperty(k2, 'motion', { enumerable: true, get: function () { return hello ? hello.motion : Object.freeze({ reduced: false }) } })
  k2.K2Error = K2Error

  // Named helpers, one per row (`surface.action` → k2.surface.action).
  /** @type {Record<string, Record<string, Function>>} */
  var groups = {}
  Object.keys(TABLE).forEach(function (verb) {
    var dot = verb.indexOf('.')
    if (dot <= 0) return
    var surface = verb.slice(0, dot)
    var action = verb.slice(dot + 1)
    if (surface in k2 && !(surface in groups)) return
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
  Object.keys(groups).forEach(function (surface) {
    k2[surface] = Object.freeze(groups[surface])
  })

  Object.defineProperty(g, 'k2', { value: Object.freeze(k2), writable: false, configurable: false, enumerable: true })
})()
