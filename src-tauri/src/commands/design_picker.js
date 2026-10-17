// K2 Design mode — in-page element picker (prd-browser-design-mode-v1 D-A11).
//
// K2-authored. Injected by the host (`eval`) into Design tabs only, never
// into plain Browser tabs. The page gets no IPC: the host pulls results by
// evaluating `window.__k2DesignPicker.drain()` and treats every value as
// untrusted (Rust clamps it with k2_core::browser_notes).
//
// Rules: no HTML-string sinks (Trusted Types pages); no <style> elements
// (strict style-src CSP) — CSSOM `style.setProperty` only; a CLOSED shadow
// root for K2's own overlay; capture-phase listeners; never patch fetch or
// frameworks.
//
// This file is a function expression. The host appends `(<navSeq>)` to
// install (idempotent) for the page load numbered `navSeq`.
(function (navSeq) {
  'use strict';
  var w = window;
  var d = document;
  var VERSION = 1;
  var MAX_QUEUE = 20;
  var MAX_HTML = 8192;
  var MAX_TEXT = 400;
  var STYLE_NAMES = [
    'display', 'position', 'width', 'height', 'margin', 'padding', 'color',
    'background-color', 'font-family', 'font-size', 'font-weight', 'line-height',
    'text-align', 'z-index', 'opacity', 'border', 'border-radius', 'box-shadow',
    'flex-direction', 'justify-content', 'align-items', 'gap',
    'grid-template-columns', 'grid-template-rows'
  ];
  var ACCENT = '#7c5cff';

  var existing = w.__k2DesignPicker;
  if (existing && existing.version === VERSION && typeof existing.setNav === 'function') {
    existing.setNav(navSeq);
    return 'ok';
  }

  var state = {
    nav: navSeq,
    armed: false,
    queue: [],
    counter: 0,
    hovered: null,
    badges: [],
    rafPending: false
  };

  // ── overlay (closed shadow root) ──────────────────────────────────────
  var host = d.createElement('div');
  host.setAttribute('data-k2-design', '');
  css(host, {
    position: 'fixed', top: '0', left: '0', width: '0', height: '0',
    overflow: 'visible', 'pointer-events': 'none', 'z-index': '2147483647',
    margin: '0', padding: '0', border: '0'
  });
  var root = host.attachShadow({ mode: 'closed' });
  var box = d.createElement('div');
  css(box, {
    position: 'fixed', display: 'none', 'pointer-events': 'none',
    border: '2px solid ' + ACCENT, background: 'rgba(124, 92, 255, 0.12)',
    'border-radius': '2px', 'box-sizing': 'border-box'
  });
  var label = d.createElement('div');
  css(label, {
    position: 'fixed', display: 'none', 'pointer-events': 'none',
    background: ACCENT, color: '#fff', font: '11px/16px ui-monospace, Menlo, monospace',
    padding: '0 4px', 'border-radius': '2px', 'white-space': 'nowrap',
    'max-width': '360px', overflow: 'hidden', 'text-overflow': 'ellipsis'
  });
  var badgeLayer = d.createElement('div');
  root.appendChild(box);
  root.appendChild(label);
  root.appendChild(badgeLayer);

  function mountHost() {
    var parent = d.documentElement || d.body;
    if (parent && host.parentNode !== parent) parent.appendChild(host);
  }
  mountHost();

  function css(el, props) {
    for (var k in props) {
      if (Object.prototype.hasOwnProperty.call(props, k)) el.style.setProperty(k, props[k]);
    }
  }

  // ── helpers ───────────────────────────────────────────────────────────
  function isOurs(node) {
    return node === host;
  }

  function eventElement(e) {
    var path = typeof e.composedPath === 'function' ? e.composedPath() : null;
    var t = path && path.length ? path[0] : e.target;
    while (t && t.nodeType !== 1) t = t.parentNode;
    if (!t || isOurs(t)) return null;
    return t;
  }

  function cssEscape(s) {
    if (w.CSS && typeof w.CSS.escape === 'function') return w.CSS.escape(s);
    return String(s).replace(/[^a-zA-Z0-9_ -￿-]/g, function (c) { return '\\' + c; })
      .replace(/^(\d)/, '\\3$1 ');
  }

  function uniqueId(el) {
    if (!el.id) return false;
    try {
      return d.querySelectorAll('#' + cssEscape(el.id)).length === 1;
    } catch (_) {
      return false;
    }
  }

  function segment(el) {
    var tag = el.tagName.toLowerCase();
    var part = tag;
    var cls = classList(el).slice(0, 2);
    for (var i = 0; i < cls.length; i++) part += '.' + cssEscape(cls[i]);
    var parent = el.parentElement;
    if (parent) {
      var same = 0;
      var index = 0;
      for (var c = parent.firstElementChild; c; c = c.nextElementSibling) {
        if (c.tagName === el.tagName) {
          same++;
          if (c === el) index = same;
        }
      }
      if (same > 1) part += ':nth-of-type(' + index + ')';
    }
    return part;
  }

  var TEST_ATTRS = ['data-testid', 'data-test-id', 'data-test', 'data-cy'];

  // A unique test-id attribute selector for el, or null.
  function testIdSelector(el) {
    for (var i = 0; i < TEST_ATTRS.length; i++) {
      var v = el.getAttribute && el.getAttribute(TEST_ATTRS[i]);
      if (!v || v.length > 120) continue;
      var sel = '[' + TEST_ATTRS[i] + '="' + String(v).replace(/["\\]/g, '\\$&') + '"]';
      try {
        if (d.querySelectorAll(sel).length === 1) return sel;
      } catch (_) { /* odd value */ }
    }
    return null;
  }

  // Robust first: a unique id, then a unique test id, then a short path
  // anchored on the nearest ancestor that has one.
  function selectorFor(el) {
    if (uniqueId(el)) return '#' + cssEscape(el.id);
    var own = testIdSelector(el);
    if (own) return own;
    var parts = [];
    var cur = el;
    var depth = 0;
    while (cur && cur.nodeType === 1 && depth < 6) {
      if (cur !== el && uniqueId(cur)) {
        parts.unshift('#' + cssEscape(cur.id));
        break;
      }
      var anchor = cur !== el ? testIdSelector(cur) : null;
      if (anchor) {
        parts.unshift(anchor);
        break;
      }
      var tag = cur.tagName.toLowerCase();
      if (tag === 'html' || tag === 'body') {
        if (!parts.length) parts.unshift(tag);
        break;
      }
      parts.unshift(segment(cur));
      cur = cur.parentElement;
      depth++;
    }
    var s = parts.join(' > ');
    return s.length > 512 ? s.slice(s.length - 512) : s;
  }

  function classList(el) {
    var raw = typeof el.className === 'string' ? el.className : el.getAttribute('class') || '';
    return raw.split(/\s+/).filter(Boolean).slice(0, 8);
  }

  function implicitRole(el) {
    var tag = el.tagName.toLowerCase();
    switch (tag) {
      case 'a': return el.hasAttribute('href') ? 'link' : null;
      case 'button': return 'button';
      case 'select': return 'combobox';
      case 'textarea': return 'textbox';
      case 'img': return 'img';
      case 'nav': return 'navigation';
      case 'main': return 'main';
      case 'header': return 'banner';
      case 'footer': return 'contentinfo';
      case 'ul': case 'ol': return 'list';
      case 'li': return 'listitem';
      case 'table': return 'table';
      case 'form': return 'form';
      case 'dialog': return 'dialog';
      case 'h1': case 'h2': case 'h3': case 'h4': case 'h5': case 'h6': return 'heading';
      case 'input': {
        var type = (el.getAttribute('type') || 'text').toLowerCase();
        if (type === 'checkbox') return 'checkbox';
        if (type === 'radio') return 'radio';
        if (type === 'range') return 'slider';
        if (type === 'button' || type === 'submit' || type === 'reset') return 'button';
        if (type === 'hidden') return null;
        return 'textbox';
      }
      default: return null;
    }
  }

  function squash(s, max) {
    return String(s || '').replace(/\s+/g, ' ').trim().slice(0, max);
  }

  function visibleText(el) {
    var t = typeof el.innerText === 'string' && el.innerText ? el.innerText : el.textContent;
    return squash(t, MAX_TEXT);
  }

  function accessibleName(el) {
    var aria = el.getAttribute('aria-label');
    if (aria) return squash(aria, 200);
    var by = el.getAttribute('aria-labelledby');
    if (by) {
      var parts = by.split(/\s+/).map(function (id) {
        var n = d.getElementById(id);
        return n ? squash(n.textContent, 200) : '';
      }).filter(Boolean);
      if (parts.length) return squash(parts.join(' '), 200);
    }
    var attrs = ['alt', 'title', 'placeholder'];
    for (var i = 0; i < attrs.length; i++) {
      var v = el.getAttribute(attrs[i]);
      if (v) return squash(v, 200);
    }
    if (el.id) {
      try {
        var lab = d.querySelector('label[for="' + cssEscape(el.id) + '"]');
        if (lab) return squash(lab.textContent, 200);
      } catch (_) { /* ignore */ }
    }
    var text = visibleText(el);
    return text ? text.slice(0, 200) : null;
  }

  function nearbyText(el) {
    var out = [];
    var seen = {};
    var own = visibleText(el);
    function add(node) {
      if (!node || out.length >= 5) return;
      var t = squash(node.textContent, 120);
      if (!t || t === own || seen[t]) return;
      seen[t] = true;
      out.push(t);
    }
    add(el.previousElementSibling);
    add(el.nextElementSibling);
    var parent = el.parentElement;
    if (parent) {
      var heading = parent.querySelector('h1, h2, h3, h4, h5, h6, label, legend');
      if (heading && heading !== el) add(heading);
      add(parent.previousElementSibling);
      add(parent.nextElementSibling);
    }
    return out;
  }

  var HEADING_SEL = 'h1, h2, h3, h4, h5, h6, [role="heading"]';

  function isHeading(n) {
    return !!(n && n.nodeType === 1 && n.matches && n.matches(HEADING_SEL));
  }

  // The nearest heading: el itself, else the closest one before it at
  // each level up (a previous sibling, or the last heading inside one).
  function nearestHeading(el) {
    if (isHeading(el)) return null;
    var cur = el;
    for (var depth = 0; cur && cur.nodeType === 1 && depth < 8; depth++) {
      var sib = cur.previousElementSibling;
      for (var k = 0; sib && k < 12; k++) {
        if (isHeading(sib)) return squash(sib.textContent, 120) || null;
        var inner = sib.querySelectorAll ? sib.querySelectorAll(HEADING_SEL) : [];
        if (inner.length) return squash(inner[inner.length - 1].textContent, 120) || null;
        sib = sib.previousElementSibling;
      }
      cur = cur.parentElement;
      if (cur && (cur.tagName === 'BODY' || cur.tagName === 'HTML')) break;
    }
    return null;
  }

  var LANDMARK_SEL = 'nav, main, header, footer, aside, form, dialog, section[aria-label], ' +
    'section[aria-labelledby], [role="navigation"], [role="main"], [role="banner"], ' +
    '[role="contentinfo"], [role="complementary"], [role="region"], [role="form"], [role="dialog"], [role="search"]';

  // The nearest landmark ancestor, e.g. `nav "Main"`, `main`, `form#signup`.
  function nearestLandmark(el) {
    var lm = null;
    try {
      lm = el.parentElement && el.parentElement.closest ? el.parentElement.closest(LANDMARK_SEL) : null;
    } catch (_) { /* old engine */ }
    if (!lm) return null;
    var out = lm.getAttribute('role') || lm.tagName.toLowerCase();
    if (lm.id) out += '#' + String(lm.id).slice(0, 64);
    var label = lm.getAttribute('aria-label');
    if (!label) {
      var by = lm.getAttribute('aria-labelledby');
      var n = by ? d.getElementById(by.split(/\s+/)[0]) : null;
      if (n) label = n.textContent;
    }
    label = squash(label, 80);
    if (label) out += ' "' + label.replace(/"/g, "'") + '"';
    return out.slice(0, 200);
  }

  // Dev builds: the nearest React component's name and its source file
  // (`_debugSource`, React 18 dev); either may be missing.
  function reactSource(el) {
    try {
      var keys = Object.keys(el);
      var key = null;
      for (var i = 0; i < keys.length; i++) {
        if (keys[i].indexOf('__reactFiber$') === 0 || keys[i].indexOf('__reactInternalInstance$') === 0) {
          key = keys[i];
          break;
        }
      }
      if (!key) return null;
      var fiber = el[key];
      var component = null;
      var where = null;
      for (var n = 0; fiber && n < 25 && !(component && where); n++) {
        var t = fiber.type;
        if (!component && typeof t === 'function') {
          var nm = t.displayName || t.name;
          if (nm && /^[A-Z]/.test(nm)) component = String(nm).slice(0, 80);
        }
        var src = fiber._debugSource;
        if (!where && src && src.fileName) {
          var file = String(src.fileName);
          var at = file.lastIndexOf('/src/');
          if (at >= 0) file = file.slice(at + 1);
          where = file + ':' + (src.lineNumber || 0);
        }
        fiber = fiber.return;
      }
      if (component && where) return (component + ' (' + where + ')').slice(0, 256);
      if (where) return where.slice(0, 256);
      if (component) return component;
    } catch (_) { /* dev-only nicety */ }
    return null;
  }

  function computedStyles(el) {
    var out = {};
    try {
      var cs = w.getComputedStyle(el);
      for (var i = 0; i < STYLE_NAMES.length; i++) {
        var v = cs.getPropertyValue(STYLE_NAMES[i]);
        if (v) out[STYLE_NAMES[i]] = String(v).slice(0, 120);
      }
    } catch (_) { /* detached */ }
    return out;
  }

  function rectOf(el) {
    var r = el.getBoundingClientRect();
    return { x: r.left, y: r.top, width: r.width, height: r.height };
  }

  function capture(el) {
    var html = '';
    try {
      html = String(el.outerHTML || '').slice(0, MAX_HTML);
    } catch (_) { /* ignore */ }
    var element = {
      selector: selectorFor(el),
      tag: el.tagName.toLowerCase(),
      classes: classList(el),
      text: visibleText(el),
      html: html,
      styles: computedStyles(el),
      rect: rectOf(el),
      nearbyText: nearbyText(el)
    };
    if (el.id) element.id = String(el.id).slice(0, 64);
    var role = el.getAttribute('role') || implicitRole(el);
    if (role) element.role = role;
    var name = accessibleName(el);
    if (name) element.name = name;
    var source = reactSource(el);
    if (source) element.source = source;
    var heading = nearestHeading(el);
    if (heading) element.heading = heading;
    var landmark = nearestLandmark(el);
    if (landmark) element.landmark = landmark;
    state.counter++;
    return {
      type: 'pick',
      pickId: 'p' + Date.now().toString(36) + '-' + state.counter,
      url: String(w.location.href),
      title: squash(d.title, 200),
      viewport: {
        width: w.innerWidth,
        height: w.innerHeight,
        scrollX: w.scrollX || 0,
        scrollY: w.scrollY || 0,
        dpr: w.devicePixelRatio || 1
      },
      element: element
    };
  }

  function enqueue(ev) {
    state.queue.push(ev);
    while (state.queue.length > MAX_QUEUE) state.queue.shift();
  }

  // ── highlight ─────────────────────────────────────────────────────────
  function showHighlight(el) {
    mountHost();
    if (!el) {
      box.style.setProperty('display', 'none');
      label.style.setProperty('display', 'none');
      return;
    }
    var r = el.getBoundingClientRect();
    css(box, {
      display: 'block', left: r.left + 'px', top: r.top + 'px',
      width: r.width + 'px', height: r.height + 'px'
    });
    var cls = classList(el).slice(0, 2);
    label.textContent = el.tagName.toLowerCase() + (cls.length ? '.' + cls.join('.') : '') +
      '  ' + Math.round(r.width) + '×' + Math.round(r.height);
    var top = r.top >= 18 ? r.top - 18 : r.bottom + 2;
    css(label, { display: 'block', left: Math.max(0, r.left) + 'px', top: top + 'px' });
  }

  // ── badges ────────────────────────────────────────────────────────────
  function layoutBadges() {
    state.rafPending = false;
    for (var i = 0; i < state.badges.length; i++) {
      var b = state.badges[i];
      var target = null;
      try {
        target = d.querySelector(b.selector);
      } catch (_) { /* bad selector */ }
      if (!target) {
        b.node.style.setProperty('display', 'none');
        continue;
      }
      var r = target.getBoundingClientRect();
      css(b.node, { display: 'block', left: Math.max(0, r.left - 9) + 'px', top: Math.max(0, r.top - 9) + 'px' });
    }
    if (state.hovered && state.armed) showHighlight(state.hovered);
  }

  function scheduleLayout() {
    if (state.rafPending) return;
    state.rafPending = true;
    (w.requestAnimationFrame || function (f) { return w.setTimeout(f, 16); })(layoutBadges);
  }

  function setBadges(list) {
    while (badgeLayer.firstChild) badgeLayer.removeChild(badgeLayer.firstChild);
    state.badges = [];
    var items = Array.isArray(list) ? list.slice(0, 50) : [];
    for (var i = 0; i < items.length; i++) {
      var item = items[i] || {};
      var node = d.createElement('div');
      node.textContent = String(item.n == null ? i + 1 : item.n).slice(0, 4);
      css(node, {
        position: 'fixed', display: 'none', 'pointer-events': 'none',
        'min-width': '18px', height: '18px', padding: '0 4px', 'box-sizing': 'border-box',
        'border-radius': '9px', background: ACCENT, color: '#fff',
        font: '600 11px/18px -apple-system, system-ui, sans-serif', 'text-align': 'center',
        'box-shadow': '0 0 0 2px #fff'
      });
      badgeLayer.appendChild(node);
      state.badges.push({ selector: String(item.selector || ''), node: node });
    }
    mountHost();
    layoutBadges();
    return state.badges.length;
  }

  // ── listeners (capture phase) ─────────────────────────────────────────
  function block(e) {
    e.preventDefault();
    e.stopPropagation();
    if (typeof e.stopImmediatePropagation === 'function') e.stopImmediatePropagation();
  }

  function onMove(e) {
    if (!state.armed) return;
    var el = eventElement(e);
    if (!el || el === state.hovered) return;
    state.hovered = el;
    showHighlight(el);
  }

  function onPress(e) {
    if (!state.armed) return;
    if (eventElement(e)) block(e);
  }

  function onClick(e) {
    if (!state.armed) return;
    var el = eventElement(e);
    if (!el) return;
    block(e);
    state.hovered = el;
    showHighlight(el);
    enqueue(capture(el));
  }

  function onKey(e) {
    if (e.key === 'Escape' && state.armed) {
      block(e);
      setArmed(false);
      enqueue({ type: 'key', key: 'escape' });
    }
    // No S hotkey (Rosson 2026-10-16): the palette button in the chrome bar
    // toggles Select. Every other key belongs to the page.
  }

  function setArmed(on) {
    state.armed = !!on;
    if (!state.armed) {
      state.hovered = null;
      showHighlight(null);
    }
    return state.armed;
  }

  var opts = { capture: true };
  w.addEventListener('mousemove', onMove, opts);
  w.addEventListener('pointerdown', onPress, opts);
  w.addEventListener('mousedown', onPress, opts);
  w.addEventListener('pointerup', onPress, opts);
  w.addEventListener('mouseup', onPress, opts);
  w.addEventListener('click', onClick, opts);
  w.addEventListener('auxclick', onPress, opts);
  w.addEventListener('keydown', onKey, opts);
  w.addEventListener('scroll', scheduleLayout, { capture: true, passive: true });
  w.addEventListener('resize', scheduleLayout, { passive: true });

  var api = {
    version: VERSION,
    setNav: function (n) {
      if (n !== state.nav) {
        state.nav = n;
        state.queue = [];
      }
      return state.nav;
    },
    arm: function (on) { return setArmed(on); },
    isArmed: function () { return state.armed; },
    drain: function () {
      var out = JSON.stringify({ nav: state.nav, events: state.queue });
      state.queue = [];
      return out;
    },
    setBadges: setBadges
  };
  if (w.__K2_DESIGN_TEST__ === true) {
    api.__test = { shadow: root, host: host, selectorFor: selectorFor };
  }
  Object.freeze(api);
  try {
    Object.defineProperty(w, '__k2DesignPicker', { value: api, configurable: true, writable: false, enumerable: false });
  } catch (_) {
    w.__k2DesignPicker = api;
  }
  return 'ok';
})
