// Sealed-frame guard: removes browser features from K2's subframes that
// those frames never need. Every subframe K2 draws is a sandboxed
// `<iframe srcdoc>` showing HTML K2 did not author (Garden widgets, HTML
// file tabs, dashboard panes, ticket briefs, Inbox mail; see
// src/renderer/lib/frame-csp.ts). Their CSP already refuses the network, but
// a few features are not governed by `connect-src` in every engine, so they
// are taken away here as well:
//   - WebRTC (RTCPeerConnection and friends);
//   - link resource hints (dns-prefetch, preconnect, prefetch, preload,
//     modulepreload, prerender);
//   - hyperlink auditing (`<a ping>` / `<area ping>`).
//
// It does nothing in a top-level frame: K2's own window keeps WebRTC (voice
// calls will use it there).
//
// Loaded in two places from this one file:
//   1. desktop: an all-frames, document-start init script on every K2 window
//      (src-tauri/src/k2_app_window.rs), so it also reaches a frame a page
//      creates for itself;
//   2. inside the frame's own head (frameSrcDoc's `scripted` profile, the
//      widget prelude), best effort for the web client, where app-level
//      scripts don't exist.
// A nested `<iframe srcdoc>` the frame creates gets this guard as its first
// script (for the web client; on desktop the init script already runs there).
// In WebKit a sandboxed frame can't script its own children at all (each has
// its own opaque origin), so a child with no script of its own has nothing to
// call. Running twice is a no-op. Plain ES5-style JS on purpose: it is
// injected as text, never bundled. Every global it touches is read from `w`,
// so tests can run it against a jsdom frame window.
;(function k2SealedFrameGuard(w) {
  'use strict'
  try {
    if (w.top === w.self) return
  } catch (e) {
    // A top we can't read is still a parent: this is a subframe.
  }

  var O = Object
  // This script's own text, so a nested `<iframe srcdoc>` the frame makes
  // carries the guard too (the in-frame copy can't reach into a child).
  var SELF_SRC = ';(' + Function.prototype.toString.call(k2SealedFrameGuard) + ')(window)'
  var NONCE_OK = /^[A-Za-z0-9+/_=-]{1,128}$/
  var DOCTYPE = /^\uFEFF?[\t\n\f\r ]*<!doctype\b[^>]*>/i
  var MARK = '__k2SealedFrameGuard'
  var WEBRTC = [
    'RTCPeerConnection',
    'webkitRTCPeerConnection',
    'RTCDataChannel',
    'RTCSessionDescription',
    'RTCIceCandidate',
    'RTCRtpSender',
    'RTCRtpReceiver',
    'RTCRtpTransceiver',
    'RTCDtlsTransport',
    'RTCIceTransport',
    'RTCSctpTransport',
    'MediaStreamTrackProcessor',
  ]
  var HINT = /^(?:dns-prefetch|preconnect|prefetch|preload|modulepreload|prerender)$/i
  var SPACE = /[\t\n\f\r ]+/

  function hintTokens(value) {
    var out = []
    var parts = String(value == null ? '' : value).split(SPACE)
    for (var i = 0; i < parts.length; i++) if (parts[i] && HINT.test(parts[i])) out.push(parts[i])
    return out
  }
  function hasHint(value) {
    return hintTokens(value).length > 0
  }
  function stripHints(value) {
    var keep = []
    var parts = String(value == null ? '' : value).split(SPACE)
    for (var i = 0; i < parts.length; i++) if (parts[i] && !HINT.test(parts[i])) keep.push(parts[i])
    return keep.join(' ')
  }

  /** Define `name` on `obj` for good: not writable, not configurable. */
  function lock(obj, name, desc) {
    desc.configurable = false
    if ('value' in desc) desc.writable = false
    try {
      O.defineProperty(obj, name, desc)
      return true
    } catch (e) {
      return false
    }
  }

  function guard(win) {
    if (!win || O.prototype.hasOwnProperty.call(win, MARK)) return
    if (!lock(win, MARK, { value: true })) return

    // ── WebRTC: gone, and can't be put back ─────────────────────────────
    for (var i = 0; i < WEBRTC.length; i++) {
      if (!lock(win, WEBRTC[i], { value: undefined, enumerable: false })) {
        try {
          delete win[WEBRTC[i]]
        } catch (e) {
          // Nothing more we can do for this name.
        }
      }
    }

    var doc = win.document
    var ElementP = win.Element && win.Element.prototype
    var LinkP = win.HTMLLinkElement && win.HTMLLinkElement.prototype
    var TokenP = win.DOMTokenList && win.DOMTokenList.prototype
    if (!doc || !ElementP) return

    var getAttr = ElementP.getAttribute
    var setAttr = ElementP.setAttribute
    var setAttrNS = ElementP.setAttributeNS
    var removeAttr = ElementP.removeAttribute
    var NodeP = win.Node && win.Node.prototype
    var removeChild = NodeP && NodeP.removeChild

    // Nested srcdoc documents get the guard as their first script. In a
    // nonce-only frame (Garden widgets) it reuses this script's nonce.
    var cs = doc.currentScript
    var nonce = cs && typeof cs.nonce === 'string' && NONCE_OK.test(cs.nonce) ? cs.nonce : ''
    var TAG =
      '<script data-k2-sealed-guard=""' + (nonce ? ' nonce="' + nonce + '"' : '') + '>' + SELF_SRC + '</scr' + 'ipt>'
    function guardedSrcdoc(html) {
      var s = String(html == null ? '' : html)
      if (s.indexOf(TAG) !== -1) return s
      var m = DOCTYPE.exec(s)
      return m ? m[0] + TAG + s.slice(m[0].length) : TAG + s
    }
    function isFrame(el) {
      return !!el && el.nodeType === 1 && String(el.localName).toLowerCase() === 'iframe'
    }
    function isLink(el) {
      return !!el && el.nodeType === 1 && String(el.localName).toLowerCase() === 'link'
    }
    function isPingable(el) {
      if (!el || el.nodeType !== 1) return false
      var n = String(el.localName).toLowerCase()
      return n === 'a' || n === 'area'
    }
    function drop(el) {
      try {
        setAttr.call(el, 'rel', stripHints(getAttr.call(el, 'rel')))
      } catch (e) {
        // Fall through to removal.
      }
      var p = el.parentNode
      if (p && removeChild) {
        try {
          removeChild.call(p, el)
        } catch (e) {
          // Already gone.
        }
      }
    }
    function scrub(el) {
      if (isLink(el) && hasHint(getAttr.call(el, 'rel'))) drop(el)
      else if (isPingable(el) && getAttr.call(el, 'ping') !== null) removeAttr.call(el, 'ping')
      else if (isFrame(el)) {
        var sd = getAttr.call(el, 'srcdoc')
        if (sd !== null && sd.indexOf(TAG) === -1) setAttr.call(el, 'srcdoc', guardedSrcdoc(sd))
      }
    }
    function sweep(node) {
      if (!node || (node.nodeType !== 1 && node.nodeType !== 9 && node.nodeType !== 11)) return
      if (node.nodeType === 1) scrub(node)
      if (typeof node.querySelectorAll !== 'function') return
      var found = node.querySelectorAll('link[rel], a[ping], area[ping], iframe[srcdoc]')
      for (var i = 0; i < found.length; i++) scrub(found[i])
    }

    // ── Attribute writes on links and anchors ───────────────────────────
    function attrName(name) {
      var s = String(name).toLowerCase()
      var c = s.indexOf(':')
      return c >= 0 ? s.slice(c + 1) : s
    }
    function filtered(el, name, value) {
      var n = attrName(name)
      if (isLink(el)) {
        if (n === 'rel') return { value: stripHints(value) }
        if (n === 'href' && hasHint(getAttr.call(el, 'rel'))) return null
      }
      if (n === 'ping' && isPingable(el)) return null
      if (n === 'srcdoc' && isFrame(el)) return { value: guardedSrcdoc(value) }
      return { value: value }
    }
    lock(ElementP, 'setAttribute', {
      value: function setAttribute(name, value) {
        var f = filtered(this, name, value)
        if (!f) return undefined
        return setAttr.call(this, name, f.value)
      },
    })
    if (setAttrNS) {
      lock(ElementP, 'setAttributeNS', {
        value: function setAttributeNS(ns, name, value) {
          var f = filtered(this, name, value)
          if (!f) return undefined
          return setAttrNS.call(this, ns, name, f.value)
        },
      })
    }

    function guardSetter(proto, prop, makeSetter) {
      if (!proto) return
      var d = O.getOwnPropertyDescriptor(proto, prop)
      if (!d || !d.set) return
      lock(proto, prop, { get: d.get, set: makeSetter(d.set, d.get), enumerable: d.enumerable })
    }
    guardSetter(LinkP, 'rel', function (set) {
      return function (v) {
        set.call(this, stripHints(v))
      }
    })
    guardSetter(LinkP, 'href', function (set) {
      return function (v) {
        if (hasHint(getAttr.call(this, 'rel'))) {
          drop(this)
          return
        }
        set.call(this, v)
      }
    })
    var pingOff = function () {
      return function () {
        // Hyperlink auditing is off in these frames.
      }
    }
    guardSetter(win.HTMLAnchorElement && win.HTMLAnchorElement.prototype, 'ping', pingOff)
    guardSetter(win.HTMLAreaElement && win.HTMLAreaElement.prototype, 'ping', pingOff)
    guardSetter(win.HTMLIFrameElement && win.HTMLIFrameElement.prototype, 'srcdoc', function (set) {
      return function (v) {
        set.call(this, guardedSrcdoc(v))
      }
    })

    // `link.relList.add('preconnect')` and friends.
    var linkRelLists = typeof win.WeakSet === 'function' ? new win.WeakSet() : null
    if (LinkP && TokenP && linkRelLists) {
      var rl = O.getOwnPropertyDescriptor(LinkP, 'relList')
      if (rl && rl.get) {
        var rlGet = rl.get
        lock(LinkP, 'relList', {
          get: function () {
            var list = rlGet.call(this)
            if (list) linkRelLists.add(list)
            return list
          },
          set: rl.set,
          enumerable: rl.enumerable,
        })
        var add = TokenP.add
        var toggle = TokenP.toggle
        var replace = TokenP.replace
        lock(TokenP, 'add', {
          value: function () {
            if (!linkRelLists.has(this)) return add.apply(this, arguments)
            var keep = []
            for (var i = 0; i < arguments.length; i++) if (!hasHint(arguments[i])) keep.push(arguments[i])
            return add.apply(this, keep)
          },
        })
        lock(TokenP, 'toggle', {
          value: function (token) {
            if (linkRelLists.has(this) && hasHint(token)) return false
            return toggle.apply(this, arguments)
          },
        })
        if (replace) {
          lock(TokenP, 'replace', {
            value: function (oldToken, newToken) {
              if (linkRelLists.has(this) && hasHint(newToken)) return false
              return replace.apply(this, arguments)
            },
          })
        }
        var tv = O.getOwnPropertyDescriptor(TokenP, 'value')
        if (tv && tv.set) {
          var tvSet = tv.set
          lock(TokenP, 'value', {
            get: tv.get,
            set: function (v) {
              tvSet.call(this, linkRelLists.has(this) ? stripHints(v) : v)
            },
            enumerable: tv.enumerable,
          })
        }
      }
    }

    // ── Anything the parser or innerHTML adds ───────────────────────────
    if (typeof win.MutationObserver === 'function') {
      var mo = new win.MutationObserver(function (records) {
        for (var i = 0; i < records.length; i++) {
          var r = records[i]
          if (r.type === 'attributes') scrub(r.target)
          else for (var j = 0; j < r.addedNodes.length; j++) sweep(r.addedNodes[j])
        }
      })
      mo.observe(doc, {
        childList: true,
        subtree: true,
        attributes: true,
        attributeFilter: ['rel', 'href', 'ping', 'srcdoc'],
      })
    }
    sweep(doc)

    // ── Frames this frame makes for itself ──────────────────────────────
    // Same-origin children (about:blank, no src) get the same treatment the
    // moment the parent reaches them through contentWindow/contentDocument.
    var IFrameP = win.HTMLIFrameElement && win.HTMLIFrameElement.prototype
    if (IFrameP) {
      var props = ['contentWindow', 'contentDocument']
      for (var k = 0; k < props.length; k++) {
        ;(function (prop) {
          var d = O.getOwnPropertyDescriptor(IFrameP, prop)
          if (!d || !d.get) return
          var get = d.get
          lock(IFrameP, prop, {
            get: function () {
              var v = get.call(this)
              try {
                var cw = prop === 'contentWindow' ? v : v && v.defaultView
                if (cw) guard(cw)
              } catch (e) {
                // Cross-origin child: nothing to reach.
              }
              return v
            },
            enumerable: d.enumerable,
          })
        })(props[k])
      }
    }
  }

  guard(w)
})(window)
