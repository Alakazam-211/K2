// BoardExport: a board as a portable file. No K2 identifiers go in it: no board id, no project, no
// k2blob: refs and no K2 person ids.
//   BoardExport.envelope({title, meta, objects, people, blobGet}) → Promise<object>   the JSON export
//     people: {isMe(key), isAgent(key), nameOf(key), viewerName}; blobGet(ref) → Promise<{mime, name, blob}>
//   BoardExport.png({world, box, scheme, imageData}) → Promise<Uint8Array>   a picture of the board (needs K2Png)
// The JSON shape (format "k2-board", version 1):
//   {format, version, title, meta, objects: [...sorted by (z, id)], people: {p1: {name, kind}}, blobs: {"sha256-<hex>": {mime, name, data}}}
// Objects are read as format 1 (older reactions turned round, list items given ids by position), without
// `batch`. Every person value (createdBy, updatedBy, reaction and vote keys, a reply's `by`) becomes a token
// p1, p2, … in order of first appearance; reaction and vote keys are taken in sorted order. An image (any
// exact k2blob: ref, or an older data: image) becomes {"blob": "sha256-<hex>"} with its bytes in `blobs`.
(function (g) {
  'use strict'
  const S = g.BoardSchema
  const REF_PREFIX = 'k2blob:'
  const isObj = (v) => !!v && typeof v === 'object' && !Array.isArray(v)
  const clone = (v) => JSON.parse(JSON.stringify(v))

  function base64(bytes) {
    let s = ''
    for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, Array.prototype.slice.call(bytes, i, i + 0x8000))
    return g.btoa(s)
  }
  function unbase64(text) { const s = g.atob(text); const out = new Uint8Array(s.length); for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i); return out }
  async function bytesOf(blob) {
    if (blob && typeof blob.arrayBuffer === 'function') return new Uint8Array(await blob.arrayBuffer())
    return new Promise((resolve, reject) => { const r = new g.FileReader(); r.onload = () => resolve(new Uint8Array(r.result)); r.onerror = () => reject(r.error); r.readAsArrayBuffer(blob) })
  }
  async function sha256(bytes) {
    const d = new Uint8Array(await g.crypto.subtle.digest('SHA-256', bytes))
    return Array.from(d, (b) => b.toString(16).padStart(2, '0')).join('')
  }

  // A stored object, read as format 1 (the same reader rules the board uses), without `batch`.
  function readShape(v) {
    const o = clone(v); delete o.batch
    for (const k of ['reactions', 'votes', 'checks', 'replies']) {
      if (!(k in o)) continue
      let c
      try { c = S.check(k, o[k], k) } catch (e) { if (!S.isRefusal(e)) throw e; continue }   // not readable: kept as stored
      if (c === null) delete o[k]; else o[k] = c
    }
    if (o.type === 'table' && Array.isArray(o.rows) && o.rows.every(Array.isArray)) S.ensureIds(o)
    return o
  }

  async function envelope({ title, meta, objects, people, blobGet }) {
    const list = objects.filter((v) => isObj(v) && typeof v.id === 'string').map(readShape)
    const z = (o) => (typeof o.z === 'number' && Number.isFinite(o.z) ? o.z : 0)
    list.sort((a, b) => z(a) - z(b) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))

    // People: the same person under any of their keys gets one token.
    const canon = (k) => (people.isMe(k) ? 'person:owner' : people.isAgent(k) ? 'agent:' + k.slice(6).toLowerCase() : k)
    const tokens = new Map(); const table = {}
    const token = (k) => {
      const c = canon(k)
      if (!tokens.has(c)) {
        const t = 'p' + (tokens.size + 1); tokens.set(c, t)
        table[t] = c === 'person:owner' ? { name: people.viewerName || 'You', kind: 'person' } : people.isAgent(c) ? { name: people.nameOf(c), kind: 'agent' } : { name: c, kind: 'person' }
      }
      return tokens.get(c)
    }
    const sorted = (m) => Object.keys(m).sort()
    for (const o of list) {
      if (typeof o.createdBy === 'string' && o.createdBy) o.createdBy = token(o.createdBy)
      if (typeof o.updatedBy === 'string' && o.updatedBy) o.updatedBy = token(o.updatedBy)
      if (isObj(o.reactions)) {
        const out = {}
        for (const k of sorted(o.reactions)) { const t = token(k); const list2 = out[t] || (out[t] = []); for (const em of o.reactions[k]) if (!list2.includes(em)) list2.push(em) }
        o.reactions = out
      }
      if (isObj(o.votes)) {
        const out = {}
        for (const k of sorted(o.votes)) { const t = token(k); out[t] = (out[t] || 0) + o.votes[k] }
        o.votes = out
      }
      if (Array.isArray(o.replies)) for (const r of o.replies) if (isObj(r) && typeof r.by === 'string' && r.by) r.by = token(r.by)
    }

    // Files: every exact k2blob: ref anywhere in an object, and an image's older data: src.
    const blobs = {}
    const fromRef = async (ref) => {
      const key = ref.slice(REF_PREFIX.length)
      if (!blobs[key]) { const got = await blobGet(ref); blobs[key] = { mime: got.mime, name: typeof got.name === 'string' ? got.name : null, data: base64(await bytesOf(got.blob)) } }
      return { blob: key }
    }
    const walk = async (v) => {
      if (typeof v === 'string') return S.isBlobRef(v) ? fromRef(v) : v
      if (Array.isArray(v)) { const out = []; for (const x of v) out.push(await walk(x)); return out }
      if (isObj(v)) { const out = {}; for (const k of Object.keys(v)) out[k] = await walk(v[k]); return out }
      return v
    }
    const objectsOut = []
    for (const o of list) {
      let n = await walk(o)
      if (n.type === 'image' && S.isDataImage(n.src)) {
        const m = /^data:(image\/[a-z]+);base64,(.*)$/.exec(n.src)
        const bytes = unbase64(m[2]); const key = 'sha256-' + await sha256(bytes)
        if (!blobs[key]) blobs[key] = { mime: m[1], name: null, data: base64(bytes) }
        n = Object.assign({}, n, { src: { blob: key } })
      }
      objectsOut.push(n)
    }
    const metaOut = {}
    for (const k of Object.keys(isObj(meta) ? meta : {})) if (!['format', 'title', 'batch'].includes(k)) metaOut[k] = clone(meta[k])
    return { format: 'k2-board', version: 1, title: String(title || 'Board'), meta: metaOut, objects: objectsOut, people: table, blobs }
  }

  // ---------- a picture of the board ----------
  // The board's own DOM, drawn through an SVG <foreignObject> onto a canvas, then encoded by K2Png from the
  // canvas pixels (canvas.toDataURL / toBlob come back empty in K2's sealed frame on WebKit).
  const XHTML = 'http://www.w3.org/1999/xhtml'
  function cssText() {
    let out = ''
    for (const sheet of Array.from(g.document.styleSheets)) { let rules = null; try { rules = sheet.cssRules } catch (_) { rules = null } if (rules) for (const r of Array.from(rules)) out += r.cssText + '\n' }
    // The page's :root rules apply to the drawing's own root instead.
    return out.replace(/:root/g, '.bx-root').replace(/(^|[\s,}])html(?=[\s,{])/g, '$1.bx-root')
  }
  function loadImage(src) {
    return new Promise((resolve, reject) => { const im = new g.Image(); im.onload = () => resolve(im); im.onerror = () => reject(new Error('the drawing didn’t load')); im.src = src })
  }
  // world: the board's world element; box: the area to draw (world units); imageData(img) → a data: URL for a
  // stored picture (or null to leave it out). Returns the PNG's bytes.
  async function png({ world, box, scheme, imageData, scale, pad }) {
    if (!g.K2Png) throw Object.assign(new Error('PNG export needs K2Png'), { code: 'unavailable' })
    const P = pad == null ? 40 : pad; const k = scale || 1
    const W = Math.max(1, Math.ceil(box.w + 2 * P)); const H = Math.max(1, Math.ceil(box.h + 2 * P))
    if (W * k * H * k > 64e6) throw Object.assign(new Error('too big'), { code: 'too_big' })
    const root = g.document.createElementNS(XHTML, 'div')
    root.setAttribute('class', 'bx-root exporting'); root.setAttribute('data-scheme', scheme || 'light')
    const inline = g.document.documentElement.getAttribute('style'); if (inline) root.setAttribute('style', inline)
    root.style.width = W + 'px'; root.style.height = H + 'px'; root.style.position = 'relative'; root.style.overflow = 'hidden'
    root.style.background = 'var(--bg)'; root.style.fontFamily = 'var(--font)'; root.style.color = 'var(--text)'
    const style = g.document.createElementNS(XHTML, 'style'); style.textContent = cssText(); root.appendChild(style)
    const copy = world.cloneNode(true)
    copy.classList.remove('lod')
    copy.style.transform = 'translate(' + (P - box.x) + 'px,' + (P - box.y) + 'px)'
    for (const el of Array.from(copy.querySelectorAll('.sel, .found'))) el.classList.remove('sel', 'found')
    const imgs = Array.from(world.querySelectorAll('img')); const copies = Array.from(copy.querySelectorAll('img'))
    for (let i = 0; i < copies.length; i++) {
      const src = copies[i].getAttribute('src') || ''
      if (src.indexOf('data:') === 0) continue
      const data = await imageData(imgs[i])
      if (data) copies[i].setAttribute('src', data); else copies[i].remove()
    }
    root.appendChild(copy)
    const xml = new g.XMLSerializer().serializeToString(root)
    const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="' + W + '" height="' + H + '"><foreignObject x="0" y="0" width="' + W + '" height="' + H + '">' + xml + '</foreignObject></svg>'
    const img = await loadImage('data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svg))
    const canvas = g.document.createElement('canvas'); canvas.width = Math.round(W * k); canvas.height = Math.round(H * k)
    const ctx = canvas.getContext('2d'); ctx.scale(k, k); ctx.drawImage(img, 0, 0, W, H)
    return g.K2Png.encode(canvas)
  }
  // A Blob as a data: URL (for pictures inside the drawing).
  function dataUrl(blob) {
    return new Promise((resolve, reject) => { const r = new g.FileReader(); r.onload = () => resolve(String(r.result)); r.onerror = () => reject(r.error); r.readAsDataURL(blob) })
  }

  g.BoardExport = { envelope, png, readShape, dataUrl, base64, sha256 }
})(window)
