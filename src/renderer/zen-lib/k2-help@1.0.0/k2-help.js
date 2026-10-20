/*! k2-help 1.0.0 (FSL-1.1-Apache-2.0), K2's widget kit: the (i) help button. Copyright 2025-2026 Alakazam Labs. */
// K2Help: put explanations behind a small (i), not on the canvas.
//
//   requires.libs: ["k2-help@1"]
//
//   el.appendChild(K2Help.tip('Boxes more than one minor behind the target.', { label: 'Version spread' }))
//   el.appendChild(K2Help.tip(longText, { label: 'Rollouts', title: 'How rollouts work' }))  // long text: a popover
//
// Or in markup (wired on load and whenever rows are redrawn):
//   <button class="k2h-btn" data-tip="Short text">i</button>
//   <button class="k2h-btn" data-help="rollouts-help">i</button>
//   <div id="rollouts-help" class="k2h-pop" hidden><p>Longer text</p></div>
//
// A tip shows on hover or keyboard focus; a click, a tap, Enter or Space
// pins it open; Esc, a tap outside or the same button closes it. A popover
// (long text, or `data-help`) opens on click, tap, Enter or Space, takes
// focus, and Esc, Close or a tap outside closes it and gives focus back to
// its (i). Both stay inside the frame (flipped above when there's no room
// below), follow the Garden's light/dark (--zen-* variables), and keep
// following a button a card redraws when it has the same `key`. All text
// goes in with textContent.
;(function () {
  'use strict'
  if (window.K2Help) return
  var d = document
  var LONG = 160

  var CSS =
    '.k2h-wrap{display:inline-flex;align-items:center;flex:none;vertical-align:middle}' +
    '.k2h-btn{flex:none;display:inline-flex;align-items:center;justify-content:center;width:18px;height:18px;margin:0 0 0 4px;padding:0;' +
    'font:inherit;font-size:12px;line-height:1;color:var(--zen-text-muted,#5f6672);background:transparent;border:0;border-radius:50%;cursor:help;' +
    '-webkit-tap-highlight-color:transparent;touch-action:manipulation}' +
    '.k2h-btn:hover,.k2h-btn[aria-expanded="true"]{color:var(--zen-accent,#2563eb)}' +
    '.k2h-btn:focus-visible{outline:2px solid var(--zen-accent,#2563eb);outline-offset:1px}' +
    '.k2h-sr{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0 0 0 0);white-space:nowrap;border:0}' +
    '.k2h-tip,.k2h-pop{position:fixed;z-index:2147483000;box-sizing:border-box;color:var(--zen-text,#1a1d23);background:var(--zen-surface-raised,#ffffff);' +
    'border:1px solid var(--zen-border,#e3e6ec);border-radius:var(--k2h-radius,8px);box-shadow:0 6px 24px rgba(0,0,0,.18);' +
    'font-family:var(--zen-font-family,-apple-system,BlinkMacSystemFont,system-ui,sans-serif);font-size:12px;line-height:1.45;font-weight:400;' +
    'text-transform:none;letter-spacing:normal;white-space:normal;text-align:left}' +
    ':root[data-zen-scheme="dark"] .k2h-tip,:root[data-zen-scheme="dark"] .k2h-pop{color:var(--zen-text,#e6e8ec);background:var(--zen-surface-raised,#222328);' +
    'border-color:var(--zen-border,#2e3036);box-shadow:0 6px 24px rgba(0,0,0,.5)}' +
    ':root[data-zen-scheme="dark"] .k2h-btn{color:var(--zen-text-muted,#9aa0ab)}' +
    ':root[data-zen-scheme="dark"] .k2h-btn:hover,:root[data-zen-scheme="dark"] .k2h-btn[aria-expanded="true"]{color:var(--zen-accent,#60a5fa)}' +
    '.k2h-tip{max-width:min(280px,calc(100vw - 16px));padding:6px 9px;pointer-events:none}' +
    '.k2h-pop{width:min(420px,calc(100vw - 16px));max-height:calc(100vh - 16px);overflow:auto;padding:8px 10px 10px}' +
    '.k2h-pop:focus{outline:none}.k2h-pop[hidden],.k2h-tip[hidden]{display:none}' +
    '.k2h-head{display:flex;align-items:center;justify-content:space-between;gap:8px;margin-bottom:4px}' +
    '.k2h-title{font-weight:600;font-size:11px;letter-spacing:.04em;color:var(--zen-text-muted,#5f6672)}' +
    ':root[data-zen-scheme="dark"] .k2h-title{color:var(--zen-text-muted,#9aa0ab)}' +
    '.k2h-pop p{margin:4px 0}' +
    '.k2h-close{font:inherit;font-size:11px;color:inherit;background:transparent;border:1px solid var(--zen-border,#e3e6ec);border-radius:4px;padding:1px 8px;cursor:pointer}' +
    ':root[data-zen-scheme="dark"] .k2h-close{border-color:var(--zen-border,#2e3036)}' +
    '.k2h-close:focus-visible{outline:2px solid var(--zen-accent,#2563eb);outline-offset:1px}'

  function addStyle() {
    var s = d.createElement('style')
    s.setAttribute('data-k2-lib', 'k2-help')
    s.textContent = CSS
    ;(d.head || d.documentElement).appendChild(s)
  }
  addStyle()

  var tipEl = null
  var tipFor = null // the button the tooltip shows for
  var tipPinned = false
  var dlg = null // { btn, pop, made }
  var lastFocusKey = null
  var seq = 0
  var touchAt = -1e9

  function host() {
    return d.body || d.documentElement
  }

  function isBtn(t) {
    return t && t.closest ? t.closest('.k2h-btn') : null
  }

  /** Below the button, or above when there's no room; inside the frame. */
  function place(btn, pop) {
    pop.style.left = '0px'
    pop.style.top = '0px'
    var r = btn.getBoundingClientRect()
    var vw = d.documentElement.clientWidth || window.innerWidth || 0
    var vh = d.documentElement.clientHeight || window.innerHeight || 0
    var pw = pop.offsetWidth
    var ph = pop.offsetHeight
    var left = Math.max(8, Math.min(r.left + r.width / 2 - pw / 2, vw - pw - 8))
    var top = r.bottom + 6
    var above = false
    if (top + ph > vh - 8 && r.top - ph - 6 >= 8) {
      top = r.top - ph - 6
      above = true
    }
    top = Math.max(8, Math.min(top, vh - ph - 8))
    pop.style.left = Math.round(left) + 'px'
    pop.style.top = Math.round(top) + 'px'
    pop.setAttribute('data-k2h-side', above ? 'above' : 'below')
  }

  function ensureTip() {
    if (tipEl && tipEl.isConnected) return tipEl
    tipEl = d.createElement('div')
    tipEl.className = 'k2h-tip'
    tipEl.setAttribute('role', 'tooltip')
    // Screen readers read each button's own description instead.
    tipEl.setAttribute('aria-hidden', 'true')
    tipEl.hidden = true
    host().appendChild(tipEl)
    return tipEl
  }

  function showTip(btn, pin) {
    closePop(false)
    var t = ensureTip()
    if (tipFor && tipFor !== btn) tipFor.setAttribute('aria-expanded', 'false')
    tipFor = btn
    tipPinned = !!pin
    t.textContent = btn.getAttribute('data-tip') || ''
    t.hidden = false
    place(btn, t)
    btn.setAttribute('aria-expanded', 'true')
  }

  function hideTip() {
    if (tipFor) tipFor.setAttribute('aria-expanded', 'false')
    tipFor = null
    tipPinned = false
    if (tipEl) tipEl.hidden = true
  }

  function popFor(btn) {
    var id = btn.getAttribute('data-help')
    return id ? d.getElementById(id) : null
  }

  function openPop(btn) {
    hideTip()
    var pop = popFor(btn)
    if (!pop) return
    if (dlg && dlg.pop === pop) {
      closePop(true)
      return
    }
    closePop(false)
    dlg = { btn: btn, pop: pop }
    if (pop.parentNode !== host()) host().appendChild(pop)
    pop.hidden = false
    place(btn, pop)
    btn.setAttribute('aria-expanded', 'true')
    pop.focus()
  }

  function closePop(refocus) {
    if (!dlg) return
    var g = dlg
    dlg = null
    g.pop.hidden = true
    g.btn.setAttribute('aria-expanded', 'false')
    if (refocus && g.btn.isConnected) g.btn.focus()
  }

  function closeAll() {
    hideTip()
    closePop(false)
  }

  /** A tip button gets its own hidden description for screen readers. */
  function wireTip(btn) {
    if (btn.__k2h) return
    btn.__k2h = true
    if (!btn.getAttribute('aria-describedby') && btn.parentNode) {
      var s = d.createElement('span')
      s.className = 'k2h-sr'
      s.id = 'k2h-d' + ++seq
      s.textContent = btn.getAttribute('data-tip') || ''
      btn.parentNode.insertBefore(s, btn.nextSibling)
      btn.setAttribute('aria-describedby', s.id)
    }
    btn.setAttribute('aria-expanded', 'false')
  }

  function wirePop(btn, own) {
    if (btn.__k2h) return
    btn.__k2h = true
    btn.setAttribute('aria-haspopup', 'dialog')
    btn.setAttribute('aria-expanded', 'false')
    var pop = own || popFor(btn)
    if (!pop) return
    btn.setAttribute('aria-controls', pop.id)
    pop.setAttribute('role', 'dialog')
    pop.setAttribute('tabindex', '-1')
    if (!pop.classList.contains('k2h-pop')) pop.classList.add('k2h-pop')
    var close = pop.querySelector('.k2h-close')
    if (close && !close.__k2h) {
      close.__k2h = true
      close.addEventListener('click', function () {
        closePop(true)
      })
    }
  }

  function wire(root) {
    var r = root || d
    if (!r.querySelectorAll) return
    var tips = r.querySelectorAll('.k2h-btn[data-tip]')
    for (var i = 0; i < tips.length; i++) wireTip(tips[i])
    var pops = r.querySelectorAll('.k2h-btn[data-help]')
    for (var j = 0; j < pops.length; j++) wirePop(pops[j])
  }

  function button(label, key) {
    var b = d.createElement('button')
    b.type = 'button'
    b.className = 'k2h-btn'
    b.textContent = '\u24D8'
    b.setAttribute('aria-label', label ? 'About ' + label : 'About this')
    if (key) b.setAttribute('data-k2h-key', String(key))
    return b
  }

  /** A popover of `text` (paragraphs at blank lines) for `b`. */
  function makePop(b, text, opts) {
    var pop = d.createElement('div')
    pop.className = 'k2h-pop'
    pop.id = 'k2h-p' + ++seq
    pop.hidden = true
    var head = d.createElement('div')
    head.className = 'k2h-head'
    var title = d.createElement('span')
    title.className = 'k2h-title'
    title.id = pop.id + '-t'
    title.textContent = opts.title || opts.label || 'About this'
    var close = d.createElement('button')
    close.type = 'button'
    close.className = 'k2h-close'
    close.textContent = 'Close'
    head.appendChild(title)
    head.appendChild(close)
    pop.appendChild(head)
    pop.setAttribute('aria-labelledby', title.id)
    String(text)
      .split(/\n\s*\n/)
      .forEach(function (para) {
        var p = d.createElement('p')
        p.textContent = para.trim()
        pop.appendChild(p)
      })
    b.setAttribute('data-help', pop.id)
    return pop
  }

  /**
   * An (i) button for rows drawn in JS. opts: {label (what it explains, for
   * "About <label>"), key (keeps it across redraws), title, popover (force a
   * popover; text over 160 characters is one anyway)}.
   */
  function tip(text, opts) {
    var o = opts && typeof opts === 'object' ? opts : { label: typeof opts === 'string' ? opts : '' }
    var b = button(o.label, o.key)
    var wrap = d.createElement('span')
    wrap.className = 'k2h-wrap'
    wrap.appendChild(b)
    if (o.popover || String(text).length > LONG) {
      var pop = makePop(b, text, o)
      wrap.appendChild(pop)
      wirePop(b, pop)
    } else {
      b.setAttribute('data-tip', String(text))
      // Its description sits next to it, so it is wired now.
      wireTip(b)
    }
    return wrap
  }

  d.addEventListener(
    'pointerdown',
    function (e) {
      if (e.pointerType === 'touch' || e.pointerType === 'pen') touchAt = Date.now()
      var b = isBtn(e.target)
      if (b) return
      if (tipEl && tipEl.contains(e.target)) return
      if (dlg && dlg.pop.contains(e.target)) return
      // A tap or click outside closes (no focus move).
      if (tipFor) hideTip()
      if (dlg) closePop(false)
    },
    true,
  )
  d.addEventListener('mouseover', function (e) {
    if (Date.now() - touchAt < 800) return // a tap's compatibility mouse events
    var b = isBtn(e.target)
    if (b && b.hasAttribute('data-tip') && !tipPinned && tipFor !== b) showTip(b, false)
  })
  d.addEventListener('mouseout', function (e) {
    var b = isBtn(e.target)
    if (b && b === tipFor && !tipPinned && !(e.relatedTarget && b.contains(e.relatedTarget))) hideTip()
  })
  d.addEventListener('focusin', function (e) {
    var b = isBtn(e.target)
    lastFocusKey = b ? b.getAttribute('data-k2h-key') : null
    if (b && b.hasAttribute('data-tip') && Date.now() - touchAt >= 800) {
      if (tipFor !== b) showTip(b, false)
    } else if (tipFor && !tipPinned && tipFor !== b) hideTip()
  })
  d.addEventListener('focusout', function (e) {
    var b = isBtn(e.target)
    if (b && b.isConnected) lastFocusKey = null // moved on (a redraw removes the button instead)
    if (b && b === tipFor && !tipPinned) hideTip()
  })
  d.addEventListener('click', function (e) {
    var b = isBtn(e.target)
    if (!b) return
    if (b.hasAttribute('data-help')) {
      openPop(b)
      return
    }
    if (b.hasAttribute('data-tip')) {
      if (tipFor === b && tipPinned) {
        hideTip()
        return
      }
      showTip(b, true)
      // WebKit doesn't focus a clicked button: focus it, so Esc reaches it.
      if (d.activeElement !== b && typeof b.focus === 'function') b.focus({ preventScroll: true })
    }
  })
  d.addEventListener('keydown', function (e) {
    if (e.key !== 'Escape' && e.key !== 'Esc') return
    if (dlg) {
      e.preventDefault()
      closePop(true)
      return
    }
    if (tipFor) {
      e.preventDefault()
      var b = tipFor
      hideTip()
      if (b.isConnected && d.activeElement !== b) b.focus()
    }
  })
  window.addEventListener('resize', function () {
    if (tipFor && tipFor.isConnected && tipEl) place(tipFor, tipEl)
    if (dlg) place(dlg.btn, dlg.pop)
  })
  d.addEventListener(
    'scroll',
    function () {
      if (tipFor && !tipPinned) hideTip()
      else if (tipFor && tipEl) place(tipFor, tipEl)
      if (dlg) place(dlg.btn, dlg.pop)
    },
    true,
  )

  // Cards redraw their rows: follow the new button with the same key, keep
  // keyboard focus on it, or close what belonged to a button that's gone.
  function sel(k) {
    return '.k2h-btn[data-k2h-key="' + String(k).replace(/["\\]/g, '') + '"]'
  }
  function onMutate() {
    wire(d)
    if (tipFor && !tipFor.isConnected) {
      var k = tipFor.getAttribute('data-k2h-key')
      var nb = k ? d.querySelector(sel(k)) : null
      var pinned = tipPinned
      hideTip()
      if (nb && pinned) showTip(nb, true)
    }
    if (dlg && !dlg.btn.isConnected) closePop(false)
    if (lastFocusKey && (d.activeElement === d.body || !d.activeElement)) {
      var fb = d.querySelector(sel(lastFocusKey))
      if (fb) fb.focus()
    }
  }
  if (typeof MutationObserver !== 'undefined') {
    new MutationObserver(onMutate).observe(d.documentElement, { childList: true, subtree: true })
  }
  if (d.readyState === 'loading') d.addEventListener('DOMContentLoaded', function () { wire(d) })
  else wire(d)

  window.K2Help = Object.freeze({
    version: '1.0.0',
    tip: tip,
    wire: wire,
    close: closeAll,
    /** What is open now: 'tip', 'pinned', 'popover' or '' (tests, layout). */
    state: function () {
      return dlg ? 'popover' : tipFor ? (tipPinned ? 'pinned' : 'tip') : ''
    },
  })
})()
