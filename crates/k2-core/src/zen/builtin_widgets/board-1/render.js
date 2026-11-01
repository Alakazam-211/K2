// BoardRender: draws the model into the world element. Text that people or
// agents wrote is always set with textContent.
(function (g) {
  'use strict'
  const S = g.BoardSchema; const G = g.BoardGeom
  const NS = 'http://www.w3.org/2000/svg'
  const svgEl = (tag, attrs) => { const e = document.createElementNS(NS, tag); for (const k in attrs || {}) e.setAttribute(k, attrs[k]); return e }
  const div = (cls, parent) => { const e = document.createElement('div'); if (cls) e.className = cls; if (parent) parent.appendChild(e); return e }

  function create(world, model) {
    let dark = false
    let votingOn = false
    const els = new Map()
    const deps = new Map()       // object id -> Set of connector ids attached to it
    const kids = new Map()       // mind parent id -> Set of child ids
    const links = svgEl('svg', { class: 'mind-links' }); world.appendChild(links)
    const measure = div('measure', document.body)

    // ---------- colours ----------
    const pick = (table, v, fallback) => (table[v] ? table[v][dark ? 1 : 0] : (typeof v === 'string' && v[0] === '#' ? v : fallback))
    const stickyColor = (v) => pick(S.STICKY, v, S.STICKY.yellow[0])
    const ink = (v) => pick(S.INK, v, dark ? '#ecebe8' : '#1f1d1a')
    const fill = (v) => (v === 'none' ? 'transparent' : pick(S.FILL, v, 'transparent'))

    function zIndex(o) {
      if (o.type === 'frame') return String(10 + Math.max(0, Math.min(9000, Math.round(o.z || 0))))
      if (o.type === 'comment') return '9000000'
      return String(10000 + Math.round(o.z || 0))
    }

    // ---------- index of connectors and mind children ----------
    function unindex(id) {
      for (const set of deps.values()) set.delete(id)
      for (const set of kids.values()) set.delete(id)
    }
    function index(o) {
      unindex(o.id)
      if (o.type === 'connector') for (const e of [o.from, o.to]) if (e && e.id) { if (!deps.has(e.id)) deps.set(e.id, new Set()); deps.get(e.id).add(o.id) }
      if (o.type === 'mind' && o.parent) { if (!kids.has(o.parent)) kids.set(o.parent, new Set()); kids.get(o.parent).add(o.id) }
    }

    // ---------- shared bits ----------
    function base(o) {
      let el = els.get(o.id)
      if (el && el.dataset.type !== o.type) { el.remove(); el = null }
      if (!el) {
        el = div('obj t-' + o.type); el.dataset.id = o.id; el.dataset.type = o.type
        world.appendChild(el); els.set(o.id, el)
      }
      return el
    }
    function place(el, o, keepHeight) {
      el.style.left = o.x + 'px'; el.style.top = o.y + 'px'
      el.style.width = o.w + 'px'
      el.style.height = keepHeight ? '' : o.h + 'px'
      el.style.transform = o.rot ? 'rotate(' + o.rot + 'deg)' : ''
      el.style.zIndex = zIndex(o)
      el.classList.toggle('locked', !!o.locked)
    }
    function child(el, cls, tag) {
      let c = el.querySelector(':scope > .' + cls)
      if (!c) { c = document.createElement(tag || 'div'); c.className = cls; el.appendChild(c) }
      return c
    }
    function setText(node, text) { if (node.textContent !== text) node.textContent = text }

    // A checklist: ticks are buttons the canvas toggles; the text is textContent only.
    function checklist(host, o) {
      const list = Array.isArray(o.checks) ? o.checks : null
      let box = host.querySelector(':scope > .checks')
      if (!list || !list.length) { if (box) box.remove(); return }
      if (!box) box = child(host, 'checks')
      while (box.children.length > list.length) box.lastChild.remove()
      while (box.children.length < list.length) { const r = div('chk', box); const b = div('box', r); b.setAttribute('role', 'checkbox'); div('ct', r) }
      list.forEach((c, i) => {
        const r = box.children[i]; r.dataset.i = i; r.classList.toggle('done', !!c.done)
        r.firstChild.setAttribute('aria-checked', String(!!c.done)); r.firstChild.setAttribute('aria-label', (c.done ? 'Done: ' : 'To do: ') + (c.text || 'item'))
        setText(r.lastChild, c.text || '')
      })
    }
    function badges(el, o) {
      // Reactions under the object: {person: [emoji, …]}, shown per emoji with who gave it.
      const byEmoji = new Map(); const r0 = o.reactions || {}
      for (const who of Object.keys(r0)) for (const em of Array.isArray(r0[who]) ? r0[who] : []) { if (!byEmoji.has(em)) byEmoji.set(em, []); byEmoji.get(em).push(who) }
      const r = {}; for (const em of S.REACTIONS) if (byEmoji.has(em)) r[em] = byEmoji.get(em)
      const all = Object.keys(r); const names = (k) => r[k].map((x) => model.nameOf(x)).join(', ')
      const keys = all.filter((k) => !S.STAMPS.includes(k)); const stamps = all.filter((k) => S.STAMPS.includes(k))
      // Stamps: the same reactions, drawn big on the top-left corner (a review mark). Click one to take yours off.
      let st = el.querySelector(':scope > .stamps')
      if (stamps.length) {
        if (!st) st = child(el, 'stamps')
        st.textContent = ''
        for (const k of stamps) {
          const s = div('react stamp', st); s.dataset.emoji = k; s.title = names(k)
          const e = document.createElement('span'); e.textContent = k; s.appendChild(e)
          if (r[k].length > 1) { const n = document.createElement('b'); n.textContent = String(r[k].length); s.appendChild(n) }
          if (r[k].some((x) => model.isMe(x))) s.classList.add('mine')
        }
      } else if (st) st.remove()
      let row = el.querySelector(':scope > .reacts')
      if (keys.length) {
        if (!row) row = child(el, 'reacts')
        row.textContent = ''
        for (const k of keys) {
          const chip = div('react', row); chip.dataset.emoji = k
          chip.title = names(k)
          const e = document.createElement('span'); e.textContent = k; chip.appendChild(e)
          const n = document.createElement('b'); n.textContent = String(r[k].length); chip.appendChild(n)
          if (r[k].some((x) => model.isMe(x))) chip.classList.add('mine')   // the same people the toggle counts as the person
        }
      } else if (row) row.remove()
      // Votes: a badge in the corner while voting is on, or when there are results.
      const v = o.votes || {}; const total = Object.values(v).reduce((a, b) => a + b, 0)
      let vb = el.querySelector(':scope > .votes')
      const votable = ['sticky', 'card'].includes(o.type)
      if ((votable && votingOn) || total) {
        if (!vb) vb = child(el, 'votes')
        vb.textContent = String(total); vb.classList.toggle('zero', !total); vb.classList.toggle('live', votingOn)
        vb.title = Object.entries(v).map(([k, n]) => model.nameOf(k) + ': ' + n).join(', ')
      } else if (vb) vb.remove()
    }
    // A link: a small badge at the top right; the address is only ever an attribute and textContent.
    function linkBadge(el, o) {
      let b = el.querySelector(':scope > .linkb')
      if (typeof o.link !== 'string' || !o.link) { if (b) b.remove(); return }
      if (!b) { b = div('linkb', el); b.setAttribute('role', 'button'); b.appendChild(g.BoardIcons ? g.BoardIcons.icon('link', 14) : document.createTextNode('↗')) }
      b.title = o.link; b.setAttribute('aria-label', 'Link: ' + o.link)
    }
    function tags(el, o, cls) {
      const list = o.tags || []
      let row = el.querySelector('.' + (cls || 'tags'))
      if (!list.length) { if (row) row.remove(); return }
      if (!row) row = child(el.querySelector('.body') || el, cls || 'tags')
      row.textContent = ''
      for (const t of list) { const c = div('tag', row); c.textContent = String(t) }
    }

    // ---------- types ----------
    const draw = {
      sticky(el, o) {
        place(el, o)
        const body = child(el, 'body'); body.style.background = stickyColor(o.color)
        const txt = child(body, 'txt'); setText(txt, o.text || '')
        txt.style.textAlign = o.align || 'center'
        tags(el, o)
        fitText(txt, o)
      },
      text(el, o) {
        place(el, o, true)
        const txt = child(el, 'txt'); setText(txt, o.text || '')
        txt.style.color = ink(o.color); txt.style.fontSize = (o.fontSize || 24) + 'px'
        txt.style.fontWeight = o.bold ? '700' : '400'; txt.style.fontStyle = o.italic ? 'italic' : 'normal'
        txt.style.textAlign = o.align || 'left'
        checklist(el, o); const ck = el.querySelector(':scope > .checks'); if (ck) ck.style.fontSize = Math.max(12, Math.round((o.fontSize || 24) * 0.75)) + 'px'
      },
      shape(el, o) {
        place(el, o)
        let svg = el.querySelector(':scope > svg')
        if (!svg) { svg = svgEl('svg', { class: 'shape-svg' }); svg.appendChild(svgEl('path')); el.prepend(svg) }
        svg.setAttribute('viewBox', '0 0 ' + o.w + ' ' + o.h); svg.setAttribute('width', o.w); svg.setAttribute('height', o.h)
        const p = svg.firstChild
        p.setAttribute('d', G.shapePath(o.shape, o.w, o.h))
        p.setAttribute('fill', fill(o.fill)); p.setAttribute('stroke', o.strokeWidth ? ink(o.stroke) : 'none')
        p.setAttribute('stroke-width', o.strokeWidth || 0)
        p.setAttribute('stroke-dasharray', dash(o.dash, o.strokeWidth))
        const tb = G.textBox(o.shape)
        const txt = child(el, 'txt')
        Object.assign(txt.style, { left: tb[0] * 100 + '%', top: tb[1] * 100 + '%', width: tb[2] * 100 + '%', height: tb[3] * 100 + '%', color: ink(o.color), fontSize: wordFit(o.text || '', o.fontSize || 18, tb[2] * o.w - 6) + 'px' })
        setText(txt, o.text || '')
      },
      connector(el, o) {
        const r = G.route(o, (id) => model.get(id))
        // An end whose object is gone: drawn faded at its stored point, or not drawn at all (never to 0, 0).
        el.classList.toggle('orphan', !!r.missing); el.hidden = !!r.unplaced
        el.setAttribute('aria-label', (r.missing ? 'Line to a deleted item' : 'Line') + (o.label ? ': ' + o.label : ''))
        if (r.unplaced) { el._route = null; el._box = null; return }
        const sw = o.strokeWidth || 2
        let x1 = Infinity; let y1 = Infinity; let x2 = -Infinity; let y2 = -Infinity
        for (const p of r.pts) { x1 = Math.min(x1, p[0]); y1 = Math.min(y1, p[1]); x2 = Math.max(x2, p[0]); y2 = Math.max(y2, p[1]) }
        const pad = 24 + sw * 3
        x1 -= pad; y1 -= pad; x2 += pad; y2 += pad
        el.style.left = x1 + 'px'; el.style.top = y1 + 'px'; el.style.width = (x2 - x1) + 'px'; el.style.height = (y2 - y1) + 'px'
        el.style.zIndex = zIndex(o); el.style.transform = ''
        el._route = r; el._box = { x: x1 + pad, y: y1 + pad, w: x2 - x1 - 2 * pad, h: y2 - y1 - 2 * pad }
        let svg = el.querySelector(':scope > svg')
        if (!svg) {
          svg = svgEl('svg', { class: 'conn-svg' })
          svg.append(svgEl('path', { class: 'hit' }), svgEl('path', { class: 'line' }), svgEl('path', { class: 'cap cap-a' }), svgEl('path', { class: 'cap cap-b' }))
          el.prepend(svg)
        }
        svg.setAttribute('viewBox', x1 + ' ' + y1 + ' ' + (x2 - x1) + ' ' + (y2 - y1))
        svg.setAttribute('width', x2 - x1); svg.setAttribute('height', y2 - y1)
        const color = ink(o.stroke)
        const [hit, line, ca, cb] = svg.children
        hit.setAttribute('d', r.d)
        line.setAttribute('d', r.d); line.setAttribute('stroke', color); line.setAttribute('stroke-width', sw)
        line.setAttribute('stroke-dasharray', dash(o.dash, sw))
        const setCap = (node, kind, p, d) => {
          const c = G.cap(kind, p, d, sw)
          if (!c) { node.setAttribute('d', ''); return }
          node.setAttribute('d', c.d); node.setAttribute('stroke', color); node.setAttribute('stroke-width', sw)
          node.setAttribute('fill', c.filled ? color : 'none')
        }
        setCap(ca, o.startCap, r.pts[0], [-r.start[0], -r.start[1]].map((v) => -v))
        setCap(cb, o.endCap, r.pts[r.pts.length - 1], r.end)
        const lbl = el.querySelector(':scope > .label')
        if (o.label) {
          const l = lbl || child(el, 'label'); const m = G.midpoint(r.pts)
          l.style.left = (m[0] - x1) + 'px'; l.style.top = (m[1] - y1) + 'px'; setText(l, o.label)
        } else if (lbl) lbl.remove()
      },
      pen(el, o) {
        place(el, o)
        let svg = el.querySelector(':scope > svg')
        if (!svg) { svg = svgEl('svg', { class: 'pen-svg' }); svg.append(svgEl('path', { class: 'hit' }), svgEl('path', { class: 'line' })); el.prepend(svg) }
        const pad = 20
        svg.setAttribute('viewBox', (-pad) + ' ' + (-pad) + ' ' + (o.w + 2 * pad) + ' ' + (o.h + 2 * pad))
        svg.style.left = -pad + 'px'; svg.style.top = -pad + 'px'
        svg.setAttribute('width', o.w + 2 * pad); svg.setAttribute('height', o.h + 2 * pad)
        const d = G.penPath(o.points || [])
        const [hit, line] = svg.children
        hit.setAttribute('d', d); hit.setAttribute('stroke-width', Math.max(14, (o.strokeWidth || 3) + 10))
        line.setAttribute('d', d); line.setAttribute('stroke', ink(o.stroke))
        line.setAttribute('stroke-width', o.highlighter ? Math.max(12, o.strokeWidth || 16) : (o.strokeWidth || 3))
        el.classList.toggle('hl', !!o.highlighter)
      },
      frame(el, o) {
        place(el, o)
        const body = child(el, 'body'); body.style.background = fill(o.fill)
        const t = child(el, 'ftitle'); setText(t, o.title || 'Frame')
      },
      card(el, o) {
        place(el, o)
        const body = child(el, 'body'); body.style.setProperty('--card', ink(o.color))
        const title = child(body, 'ctitle'); setText(title, o.title || '')
        const desc = child(body, 'cdesc'); setText(desc, o.text || '')
        checklist(body, o); const ck = body.querySelector(':scope > .checks'); if (ck && ck.nextSibling !== body.querySelector(':scope > .cfoot') && body.querySelector(':scope > .cfoot')) body.insertBefore(ck, body.querySelector(':scope > .cfoot'))
        const foot = child(body, 'cfoot'); foot.textContent = ''
        if (Array.isArray(o.checks) && o.checks.length) { const n = div('ckn', foot); n.textContent = o.checks.filter((c) => c.done).length + '/' + o.checks.length; n.classList.toggle('all', o.checks.every((c) => c.done)) }
        if (o.assignee) { const a = div('who', foot); const dot = div('av', a); dot.textContent = initials(o.assignee); const n = document.createElement('span'); n.textContent = o.assignee; a.appendChild(n) }
        if (o.due) { const d = div('due', foot); d.textContent = o.due }
        if (o.status) { const s = div('status', foot); s.textContent = o.status }
        tags(el, o)
      },
      table(el, o) {
        place(el, o, true)
        const rows = o.rows && o.rows.length ? o.rows : [['']]
        const cols = Math.max(...rows.map((r) => r.length))
        const grid = child(el, 'grid')
        grid.style.gridTemplateColumns = Array.from({ length: cols }, (_, i) => (o.colW && o.colW[i] ? o.colW[i] + 'px' : '1fr')).join(' ')
        grid.style.background = fill(o.fill)
        const need = rows.length * cols
        while (grid.children.length > need) grid.lastChild.remove()
        while (grid.children.length < need) div('cell', grid)
        rows.forEach((r, ri) => { for (let ci = 0; ci < cols; ci++) { const c = grid.children[ri * cols + ci]; c.dataset.r = ri; c.dataset.c = ci; c.classList.toggle('head', ri === 0); setText(c, r[ci] || '') } })
      },
      mind(el, o) {
        place(el, o)
        const pill = child(el, 'pill'); pill.style.setProperty('--mind', ink(o.color))
        pill.classList.toggle('root', !o.parent)
        const txt = child(pill, 'txt'); setText(txt, o.text || '')
      },
      comment(el, o) {
        el.style.left = o.x + 'px'; el.style.top = o.y + 'px'; el.style.width = ''; el.style.height = ''; el.style.zIndex = zIndex(o)
        el.classList.toggle('resolved', !!o.resolved)
        const pin = child(el, 'pin'); pin.textContent = ''
        const av = div('av', pin); av.textContent = initials(model.nameOf(o.createdBy) || '?')
        el.setAttribute('aria-label', 'Comment by ' + model.nameOf(o.createdBy) + (o.text ? ': ' + String(o.text).slice(0, 80) : ''))
        const n = (o.replies || []).length
        if (n) { const c = div('count', pin); c.textContent = String(n + 1) }
      },
      image(el, o) {
        place(el, o)
        let img = el.querySelector(':scope > img')
        if (!img) { img = document.createElement('img'); img.draggable = false; el.prepend(img) }
        const src = typeof o.src === 'string' ? o.src : ''
        if (S.isBlobRef(src)) {
          // A stored file: its URL comes from the host, as a thumbnail sized to the box on screen (crop included).
          const c0 = o.crop && typeof o.crop === 'object' ? o.crop : { w: 1, h: 1 }
          const need = Math.max(o.w / c0.w, o.h / c0.h) * imageScale() * (g.devicePixelRatio || 1)
          const px = need <= 256 ? 256 : need <= 512 ? 512 : 0
          const key = src + '|' + px
          if (img.dataset.key !== key && imageSource) {
            img.dataset.key = key; el.classList.add('loading'); el.classList.remove('broken')
            imageSource(src, px).then((url) => { if (img.dataset.key === key) { img.setAttribute('src', url); el.classList.remove('loading') } },
              () => { if (img.dataset.key === key) { el.classList.remove('loading'); el.classList.add('broken') } })
          }
        } else {
          // An older picture kept inside the object (a data: image): still shown as it is.
          delete img.dataset.key
          if (img.getAttribute('src') !== src) { if (src) img.setAttribute('src', src); else img.removeAttribute('src') }
        }
        img.alt = o.alt || ''
        el.setAttribute('aria-label', 'Image' + (o.alt ? ': ' + o.alt : ''))
        // A crop: the box shows that part of the picture, scaled to fill it.
        // Like the whole picture (cover), never stretched: the crop's part is scaled to fill the box and centred.
        const c = o.crop && typeof o.crop === 'object' ? o.crop : null
        el.classList.toggle('cropped', !!c)
        const fit = () => {
          const nw = img.naturalWidth; const nh = img.naturalHeight; if (!c || !nw || !nh) return
          const rw = c.w * nw; const rh = c.h * nh; const k = Math.max(o.w / rw, o.h / rh)
          const w = nw * k; const h = nh * k; const l = -(c.x * nw * k) - (rw * k - o.w) / 2; const t = -(c.y * nh * k) - (rh * k - o.h) / 2
          Object.assign(img.style, { width: w + 'px', height: h + 'px', left: l + 'px', top: t + 'px', clipPath: 'inset(' + (-t) + 'px ' + (w + l - o.w) + 'px ' + (h + t - o.h) + 'px ' + (-l) + 'px round 4px)' })
        }
        if (c) { if (img.complete && img.naturalWidth) fit(); else img.onload = fit }
        else { img.onload = null; Object.assign(img.style, { width: '', height: '', left: '', top: '', clipPath: '' }) }
      },
    }
    // Images stored as files: the host turns a ref into a URL (k2 blobs in the Garden).
    let imageSource = null; let imageScale = () => 1
    function dash(kind, sw) { sw = sw || 2; return kind === 'dashed' ? (sw * 4) + ' ' + (sw * 3) : kind === 'dotted' ? '0.1 ' + (sw * 2.5) : '' }
    function initials(name) { return String(name).split(/[\s\-_]+/).filter(Boolean).slice(0, 2).map((s) => s[0].toUpperCase()).join('') || '?' }

    // A label never breaks inside a word: the size comes down until its longest word fits the box's width.
    const ruler = document.createElement('canvas').getContext('2d'); let rulerFont = null
    function wordFit(text, size, width) {
      const words = String(text).split(/\s+/).filter(Boolean); if (!words.length || width <= 0) return size
      if (!rulerFont) rulerFont = getComputedStyle(document.body).fontFamily || 'sans-serif'   // cached: read again on a theme change (refreshFont)
      ruler.font = '500 ' + size + 'px ' + rulerFont
      const widest = Math.max(...words.map((w) => ruler.measureText(w).width))
      return widest <= width ? size : Math.max(9, Math.floor(size * width / widest))
    }

    // Sticky text: auto-size (fontSize 0) shrinks to fit, like paper notes.
    let fitQueue = null   // while all() draws, sticky text is fitted after every node is in (one layout, not one per sticky)
    function fitText(txt, o) {
      if (fitQueue) { fitQueue.push([txt, o]); return }
      if (o.fontSize) { delete txt.dataset.fit; txt.style.fontSize = o.fontSize + 'px'; return }
      const key = (o.text || '') + '|' + o.w + '|' + o.h
      if (txt.dataset.fit === key) return
      txt.dataset.fit = key
      const availW = o.w - 28; const availH = o.h - 28
      measure.style.width = availW + 'px'
      measure.textContent = o.text || 'M'
      let lo = 8; let hi = Math.min(56, Math.round(Math.min(o.h, o.w * 1.2) / 6.2)); let best = lo
      while (lo <= hi) {
        const mid = (lo + hi) >> 1
        measure.style.fontSize = mid + 'px'
        if (measure.scrollHeight <= availH && measure.scrollWidth <= availW + 1) { best = mid; lo = mid + 1 } else hi = mid - 1
      }
      txt.style.fontSize = best + 'px'
    }

    // Many stickies at once: every sticky's binary search steps together, so each step is ONE layout
    // for all of them (about 6 in all), not one per sticky per step. Same measure, same sizes as fitText.
    function fitMany(items) {
      const probes = []
      for (const [txt, o] of items) {
        if (o.fontSize) { fitText(txt, o); continue }
        const key = (o.text || '') + '|' + o.w + '|' + o.h
        if (txt.dataset.fit === key) continue
        txt.dataset.fit = key
        const el = document.createElement('div'); el.className = 'measure'
        el.style.width = (o.w - 28) + 'px'; el.textContent = o.text || 'M'; measure.parentNode.appendChild(el)
        probes.push({ txt, el, availW: o.w - 28, availH: o.h - 28, lo: 8, hi: Math.min(56, Math.round(Math.min(o.h, o.w * 1.2) / 6.2)), best: 8 })
      }
      let live = probes.filter((q) => q.lo <= q.hi)
      while (live.length) {
        for (const q of live) q.el.style.fontSize = ((q.lo + q.hi) >> 1) + 'px'   // all writes, then all reads: one layout
        for (const q of live) { const mid = (q.lo + q.hi) >> 1; if (q.el.scrollHeight <= q.availH && q.el.scrollWidth <= q.availW + 1) { q.best = mid; q.lo = mid + 1 } else q.hi = mid - 1 }
        live = live.filter((q) => q.lo <= q.hi)
      }
      for (const q of probes) { q.txt.style.fontSize = q.best + 'px'; q.el.remove() }
    }

    // ---------- mind-map branches ----------
    function drawLinks() {
      links.textContent = ''
      for (const o of model.all()) {
        if (o.type !== 'mind' || !o.parent) continue
        const p = model.get(o.parent); if (!p) continue
        const pc = [p.x + p.w / 2, p.y + p.h / 2]; const oc = [o.x + o.w / 2, o.y + o.h / 2]
        const right = oc[0] >= pc[0]
        const a = [right ? p.x + p.w : p.x, pc[1]]; const b = [right ? o.x : o.x + o.w, oc[1]]
        const k = Math.max(30, Math.abs(b[0] - a[0]) * 0.5) * (right ? 1 : -1)
        links.appendChild(svgEl('path', { d: `M${a[0]} ${a[1]}C${a[0] + k} ${a[1]} ${b[0] - k} ${b[1]} ${b[0]} ${b[1]}`, stroke: ink(o.color), 'stroke-width': p.parent ? 2.5 : 3.5, fill: 'none', 'stroke-linecap': 'round' }))
      }
    }

    // ---------- public ----------
    function drawOne(id) {
      const o = model.get(id)
      if (!o) { const el = els.get(id); if (el) { el.remove(); els.delete(id) } unindex(id); return }
      index(o)
      const el = base(o)
      draw[o.type] && draw[o.type](el, o)
      if (o.type !== 'connector' && o.type !== 'comment') { badges(el, o); linkBadge(el, o) }
    }
    function update(ids) {
      const todo = new Set(ids); let mind = false
      for (const id of ids) {
        for (const c of deps.get(id) || []) todo.add(c)
        const o = model.get(id); if (!o || o.type === 'mind') mind = true
      }
      // Connectors last, so they see their ends' new places.
      const list = [...todo].sort((a, b) => ((model.get(a) || {}).type === 'connector') - ((model.get(b) || {}).type === 'connector'))
      for (const id of list) drawOne(id)
      if (mind) drawLinks()
    }
    function all() {
      for (const [id, el] of els) if (!model.get(id)) { el.remove(); els.delete(id) }
      const list = model.all().sort((a, b) => (a.type === 'connector') - (b.type === 'connector'))
      for (const o of list) index(o)
      fitQueue = []
      try { for (const o of list) drawOne(o.id) } finally { const q = fitQueue; fitQueue = null; fitMany(q) }
      drawLinks()
    }
    // The box an object occupies, in world units (connectors and text measure themselves).
    function box(id) {
      const o = model.get(id); if (!o) return null
      const el = els.get(id)
      if (o.type === 'connector') return el && el._box ? Object.assign({}, el._box) : null
      if (o.type === 'text' || o.type === 'table') return { x: o.x, y: o.y, w: o.w, h: el ? el.offsetHeight : o.h }
      if (o.type === 'comment') return { x: o.x, y: o.y, w: 28, h: 28 }
      return { x: o.x || 0, y: o.y || 0, w: o.w || 0, h: o.h || 0 }
    }
    return {
      all, update, box,
      el: (id) => els.get(id),
      setScheme(isDark) { rulerFont = null; if (dark !== isDark) { dark = isDark; for (const el of els.values()) { const t = el.querySelector('.txt'); if (t) delete t.dataset.fit } all() } },
      refreshFont() { rulerFont = null; all() },   // the theme's font changed: shape labels are measured again
      setVoting(on) { if (votingOn !== on) { votingOn = on; all() } },
      colors: { stickyColor, ink, fill },
      // source(ref, px) → Promise<url>; scale() → the camera's zoom (thumbnails follow the size on screen).
      setImageSource(source, scale) { imageSource = source; if (scale) imageScale = scale; for (const o of model.all()) if (o.type === 'image') drawOne(o.id) },
      refreshImages() { for (const o of model.all()) if (o.type === 'image' && S.isBlobRef(o.src)) drawOne(o.id) },
      connectorsOf: (id) => deps.get(id) || new Set(),
      childrenOf: (id) => kids.get(id) || new Set(),
    }
  }
  g.BoardRender = { create }
})(window)
