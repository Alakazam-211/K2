// k2:canvas@1, the canvas (the Compute Garden, Rosson 2026-11-03): a page
// of tiles the person shows or hides. K2 ships the tiles; each one is an
// entry in this file's registry, so a later tile drops in as one more
// tile-<id>.js (its `K2Canvas.tile({...})`) and tile-<id>.css, loaded by
// index.html before start.js.
//
// A tile entry:
//   id           lower case, stable: the person's saves name it
//   name         its title, and its line in the Tiles menu
//   description  one sentence under its name in the Tiles menu
//   size         'full' (a row of its own) or 'half' (beside another)
//   shown        false: hidden until the person shows it (default true)
//   needs        the K2 verbs it calls; one this K2 doesn't offer here
//                shows a note in the tile instead of a broken one
//   mount(ctx)   draws the tile when it is shown; returns {stop()} to stop
//                its timers and subscriptions when it is hidden
//
// What a tile gets (ctx): body (its element), setMeta(text) (the title
// bar's right side), note(text) (a line above its body; null clears),
// can(verb), visible() (shown and on screen), onVisible(fn), and save.get
// / save.set (its own private saves, `tile/<id>/<key>`).
//
// Which tiles a Garden offers, and in what order: `tiles = "a,b"` under
// [widget.props.config] (default: every tile, in registry order); the
// title bar's name: `title`. Which ones the person shows is their own
// private save on this computer (k2.save `tiles`): {v, hidden, known}. A
// tile they have never seen (not in `known`) starts as its entry says, so a
// tile added in a later K2 appears without moving the ones they hid.
//
// Every value is drawn with textContent; agents' and nodes' text is data.
(function () {
  'use strict'
  var k2 = window.k2
  var SAVE = 'tiles'
  var ID = /^[a-z][a-z0-9-]{0,39}$/

  var registry = []
  var offered = []
  var hidden = []
  var known = []
  var live = {}
  var onScreen = true
  var started = false

  var $ = function (id) { return document.getElementById(id) }
  function el(tag, cls, text) {
    var e = document.createElement(tag)
    if (cls) e.className = cls
    if (text != null) e.textContent = String(text)
    return e
  }
  function can(verb) {
    return !!(k2 && typeof k2.can === 'function' && k2.can(verb))
  }
  function list(v) {
    return typeof v === 'string' ? v.split(',').map(function (s) { return s.trim() }).filter(Boolean) : []
  }

  var toastTimer = 0
  function toast(text) {
    var t = $('toast')
    t.textContent = text
    t.classList.add('cv-on')
    clearTimeout(toastTimer)
    toastTimer = setTimeout(function () { t.classList.remove('cv-on') }, 3200)
  }

  // ---- the registry --------------------------------------------------------

  function tile(def) {
    if (!def || !ID.test(String(def.id)) || typeof def.mount !== 'function') throw new Error('K2Canvas.tile: an id and a mount function')
    if (registry.some(function (t) { return t.id === def.id })) throw new Error('K2Canvas.tile: ' + def.id + ' twice')
    registry.push({
      id: def.id,
      name: String(def.name || def.id),
      description: String(def.description || ''),
      size: def.size === 'half' ? 'half' : 'full',
      shown: def.shown !== false,
      needs: Array.isArray(def.needs) ? def.needs.slice() : [],
      mount: def.mount,
    })
  }

  // ---- saves (this person, this computer) -----------------------------------

  function save() {
    if (!can('save.set')) return
    var doc = { v: 1, hidden: hidden.slice(), known: known.slice() }
    Promise.resolve().then(function () { return k2.save.set(SAVE, doc) }).catch(function () {
      // Kept for this session; the next change tries again.
    })
  }

  function load() {
    if (!can('save.get')) return Promise.resolve(null)
    return Promise.resolve().then(function () { return k2.save.get(SAVE) }).catch(function () { return null })
  }

  function strings(v) {
    return Array.isArray(v) ? v.filter(function (s) { return typeof s === 'string' && ID.test(s) }) : []
  }

  function isShown(t) {
    return hidden.indexOf(t.id) < 0
  }

  // ---- drawing --------------------------------------------------------------

  function ctxFor(t, parts) {
    var visibleFns = []
    var ctx = {
      body: parts.body,
      setMeta: function (text) { parts.meta.textContent = text == null ? '' : String(text) },
      note: function (text) {
        parts.note.textContent = text == null ? '' : String(text)
        parts.note.hidden = text == null || text === ''
      },
      can: can,
      visible: function () { return onScreen && !document.hidden && !!live[t.id] },
      onVisible: function (fn) { visibleFns.push(fn) },
      save: {
        get: function (key) {
          if (!can('save.get')) return Promise.resolve(null)
          return Promise.resolve().then(function () { return k2.save.get('tile/' + t.id + '/' + key) })
        },
        set: function (key, value) {
          if (!can('save.set')) return Promise.resolve(null)
          return Promise.resolve().then(function () { return k2.save.set('tile/' + t.id + '/' + key, value) })
        },
      },
      toast: toast,
    }
    return { ctx: ctx, visibleFns: visibleFns }
  }

  function mount(t) {
    var box = el('section', 'cv-tile cv-' + t.size)
    box.setAttribute('aria-label', t.name)
    box.dataset.tile = t.id
    var head = el('header', 'cv-tile-head')
    head.appendChild(el('h2', 'cv-tile-name', t.name))
    var meta = el('span', 'cv-tile-meta')
    head.appendChild(meta)
    var hide = el('button', 'cv-x', '×')
    hide.type = 'button'
    hide.title = 'Hide ' + t.name + ' (Tiles shows it again)'
    hide.setAttribute('aria-label', 'Hide ' + t.name)
    hide.addEventListener('click', function () { setShown(t.id, false) })
    head.appendChild(hide)
    box.appendChild(head)
    var note = el('p', 'cv-tile-note')
    note.hidden = true
    box.appendChild(note)
    var body = el('div', 'cv-tile-body')
    box.appendChild(body)
    var made = ctxFor(t, { body: body, meta: meta, note: note })
    var entry = { el: box, stop: null, visibleFns: made.visibleFns }
    live[t.id] = entry
    var missing = t.needs.filter(function (v) { return !can(v) })
    if (missing.length) {
      // In words for the person; the verbs it needs for whoever looks closer.
      var why = el('p', 'cv-empty', t.name + ' isn’t available here yet.')
      why.title = 'It needs ' + missing.join(', ') + ', which this K2 doesn’t offer here.'
      body.appendChild(why)
      return box
    }
    try {
      var r = t.mount(made.ctx)
      entry.stop = r && typeof r.stop === 'function' ? r.stop : null
    } catch (e) {
      body.textContent = ''
      body.appendChild(el('p', 'cv-empty', t.name + ' didn’t start: ' + (e && e.message ? e.message : 'error')))
    }
    return box
  }

  function unmount(id) {
    var entry = live[id]
    if (!entry) return
    delete live[id]
    if (entry.stop) {
      try { entry.stop() } catch (e) { /* a tile that fails to stop is gone anyway */ }
    }
    entry.el.remove()
  }

  function layout() {
    var grid = $('grid')
    offered.forEach(function (t) {
      if (isShown(t) && !live[t.id]) mount(t)
      if (!isShown(t) && live[t.id]) unmount(t.id)
    })
    // Keep the offered order.
    offered.forEach(function (t) {
      if (live[t.id]) grid.appendChild(live[t.id].el)
    })
    $('empty').hidden = offered.some(isShown)
    drawMenu()
  }

  function setShown(id, show) {
    var i = hidden.indexOf(id)
    if (show && i >= 0) hidden.splice(i, 1)
    if (!show && i < 0) hidden.push(id)
    save()
    layout()
    var t = offered.filter(function (x) { return x.id === id })[0]
    if (t && !show) toast(t.name + ' hidden. Tiles shows it again.')
  }

  // ---- the Tiles menu ---------------------------------------------------------

  var menuOpen = false

  function drawMenu() {
    var menu = $('tiles-menu')
    menu.textContent = ''
    offered.forEach(function (t) {
      var item = el('button', 'cv-item')
      item.type = 'button'
      item.setAttribute('role', 'menuitemcheckbox')
      item.setAttribute('aria-checked', isShown(t) ? 'true' : 'false')
      item.dataset.tile = t.id
      item.appendChild(el('span', 'cv-check', isShown(t) ? '✓' : ''))
      var words = el('span', 'cv-item-words')
      words.appendChild(el('span', 'cv-item-name', t.name))
      if (t.description) words.appendChild(el('span', 'cv-item-desc', t.description))
      item.appendChild(words)
      item.addEventListener('click', function () { setShown(t.id, !isShown(t)) })
      menu.appendChild(item)
    })
  }

  function openMenu(open) {
    menuOpen = open
    $('tiles-menu').hidden = !open
    $('tiles-btn').setAttribute('aria-expanded', open ? 'true' : 'false')
    shareBar()
    if (open) {
      var first = $('tiles-menu').querySelector('button')
      if (first) first.focus()
    }
  }

  // On a full-canvas Garden K2's top band is the window's drag strip: the
  // Tiles button (and the open menu) are holes in it, so clicks reach them.
  function shareBar() {
    if (!can('canvas.setHitRegions')) return
    var rects = []
    ;[$('tiles-btn'), menuOpen ? $('tiles-menu') : null].forEach(function (e) {
      if (!e || e.hidden) return
      var r = e.getBoundingClientRect()
      if (r.width > 0 && r.height > 0) rects.push({ x: r.left, y: r.top, width: r.width, height: r.height })
    })
    Promise.resolve().then(function () { return k2.canvas.setHitRegions(rects) }).catch(function () {
      // The strip keeps its last holes.
    })
  }

  function wireMenu() {
    $('tiles-btn').addEventListener('click', function () { openMenu(!menuOpen) })
    document.addEventListener('keydown', function (e) {
      if (e.key === 'Escape' && menuOpen) {
        openMenu(false)
        $('tiles-btn').focus()
      }
    })
    document.addEventListener('mousedown', function (e) {
      if (!menuOpen) return
      var wrap = $('tiles-btn').parentNode
      if (!wrap.contains(e.target)) openMenu(false)
    })
  }

  // ---- start ------------------------------------------------------------------

  function visibility(v) {
    onScreen = v
    Object.keys(live).forEach(function (id) {
      live[id].visibleFns.forEach(function (fn) {
        try { fn(v && !document.hidden) } catch (e) { /* one tile never stops the others */ }
      })
    })
  }

  function start() {
    if (started) return
    started = true
    wireMenu()
    if (!k2) {
      $('empty').textContent = 'K2 didn’t start this widget.'
      $('empty').hidden = false
      return
    }
    // Once, at load: the frame's on-screen state and the page's.
    if (typeof k2.onVisible === 'function') k2.onVisible(visibility)
    document.addEventListener('visibilitychange', function () { visibility(onScreen) })
    window.addEventListener('resize', shareBar)
    k2.connected.then(function () {
      var cfg = k2.config || {}
      if (typeof cfg.title === 'string' && cfg.title.trim()) $('title').textContent = cfg.title.trim().slice(0, 60)
      var want = list(cfg.tiles)
      offered = want.length
        ? want.map(function (id) { return registry.filter(function (t) { return t.id === id })[0] }).filter(Boolean)
        : registry.slice()
      return load().then(function (doc) {
        var d = doc && typeof doc === 'object' ? doc : {}
        hidden = strings(d.hidden)
        known = strings(d.known)
        var changed = false
        offered.forEach(function (t) {
          if (known.indexOf(t.id) >= 0) return
          known.push(t.id)
          changed = true
          if (!t.shown && hidden.indexOf(t.id) < 0) hidden.push(t.id)
        })
        if (changed) save()
        layout()
        shareBar()
        k2.ready()
      })
    }).catch(function () {
      $('empty').textContent = 'K2 didn’t start this widget.'
      $('empty').hidden = false
    })
  }

  window.K2Canvas = {
    tile: tile,
    start: start,
    // For tests: the tiles offered, shown and hidden right now.
    state: function () {
      return {
        offered: offered.map(function (t) { return t.id }),
        shown: offered.filter(isShown).map(function (t) { return t.id }),
        hidden: hidden.slice(),
        known: known.slice(),
      }
    },
  }
})()
