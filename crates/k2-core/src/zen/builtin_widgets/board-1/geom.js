// BoardGeom: shape outlines, connector routes, arrowheads and boxes.
(function (g) {
  'use strict'
  const f = (n) => Math.round(n * 10) / 10
  const P = (pts) => 'M' + pts.map((p) => f(p[0]) + ' ' + f(p[1])).join('L') + 'Z'

  // SVG path for a shape filling a w×h box.
  function shapePath(kind, w, h) {
    const r = Math.min(w, h)
    switch (kind) {
      case 'rect': case 'flow-process': return P([[0, 0], [w, 0], [w, h], [0, h]])
      case 'round': { const k = Math.min(16, r / 4); return `M${k} 0H${w - k}Q${w} 0 ${w} ${k}V${h - k}Q${w} ${h} ${w - k} ${h}H${k}Q0 ${h} 0 ${h - k}V${k}Q0 0 ${k} 0Z` }
      case 'flow-start': { const k = h / 2; return `M${k} 0H${w - k}A${k} ${k} 0 0 1 ${w - k} ${h}H${k}A${k} ${k} 0 0 1 ${k} 0Z` }
      case 'ellipse': return `M0 ${h / 2}A${w / 2} ${h / 2} 0 1 0 ${w} ${h / 2}A${w / 2} ${h / 2} 0 1 0 0 ${h / 2}Z`
      case 'triangle': return P([[w / 2, 0], [w, h], [0, h]])
      case 'diamond': case 'flow-decision': return P([[w / 2, 0], [w, h / 2], [w / 2, h], [0, h / 2]])
      case 'parallelogram': case 'flow-data': { const k = w * 0.18; return P([[k, 0], [w, 0], [w - k, h], [0, h]]) }
      case 'trapezoid': { const k = w * 0.18; return P([[k, 0], [w - k, 0], [w, h], [0, h]]) }
      case 'pentagon': return P(poly(5, w, h, -90))
      case 'hexagon': { const k = w * 0.22; return P([[k, 0], [w - k, 0], [w, h / 2], [w - k, h], [k, h], [0, h / 2]]) }
      case 'octagon': { const a = w * 0.29; const b = h * 0.29; return P([[a, 0], [w - a, 0], [w, b], [w, h - b], [w - a, h], [a, h], [0, h - b], [0, b]]) }
      case 'star': {
        const pts = []
        for (let i = 0; i < 10; i++) { const ang = (-90 + i * 36) * Math.PI / 180; const rr = i % 2 ? 0.42 : 1; pts.push([w / 2 + Math.cos(ang) * w / 2 * rr, h * 0.53 + Math.sin(ang) * h * 0.53 * rr]) }
        return P(pts)
      }
      case 'cross': { const a = w / 3; const b = h / 3; return P([[a, 0], [w - a, 0], [w - a, b], [w, b], [w, h - b], [w - a, h - b], [w - a, h], [a, h], [a, h - b], [0, h - b], [0, b], [a, b]]) }
      case 'arrow-right': { const k = Math.min(w * 0.4, h * 0.6); return P([[0, h * 0.25], [w - k, h * 0.25], [w - k, 0], [w, h / 2], [w - k, h], [w - k, h * 0.75], [0, h * 0.75]]) }
      case 'arrow-left': { const k = Math.min(w * 0.4, h * 0.6); return P([[w, h * 0.25], [k, h * 0.25], [k, 0], [0, h / 2], [k, h], [k, h * 0.75], [w, h * 0.75]]) }
      case 'chevron': { const k = Math.min(w * 0.3, h * 0.5); return P([[0, 0], [w - k, 0], [w, h / 2], [w - k, h], [0, h], [k, h / 2]]) }
      case 'cylinder': { const e = Math.min(h * 0.12, 24); return `M0 ${e}A${w / 2} ${e} 0 0 1 ${w} ${e}V${h - e}A${w / 2} ${e} 0 0 1 0 ${h - e}Z M0 ${e}A${w / 2} ${e} 0 0 0 ${w} ${e}` }
      case 'cloud': return `M${w * 0.25} ${h * 0.85}C${w * 0.02} ${h * 0.85} ${w * 0.02} ${h * 0.5} ${w * 0.2} ${h * 0.47}C${w * 0.15} ${h * 0.2} ${w * 0.45} ${h * 0.08} ${w * 0.55} ${h * 0.28}C${w * 0.65} ${h * 0.1} ${w * 0.92} ${h * 0.2} ${w * 0.83} ${h * 0.45}C${w * 1.02} ${h * 0.5} ${w * 0.98} ${h * 0.85} ${w * 0.75} ${h * 0.85}Z`
      case 'speech': { const k = Math.min(14, r / 5); const b = h * 0.78; return `M${k} 0H${w - k}Q${w} 0 ${w} ${k}V${b - k}Q${w} ${b} ${w - k} ${b}H${w * 0.4}L${w * 0.2} ${h}L${w * 0.24} ${b}H${k}Q0 ${b} 0 ${b - k}V${k}Q0 0 ${k} 0Z` }
      case 'document': { const k = h * 0.12; return `M0 0H${w}V${h - k}C${w * 0.75} ${h - 2.4 * k} ${w * 0.5} ${h + k} ${w * 0.25} ${h - k * 0.4}S0 ${h - k} 0 ${h - k}Z` }
      case 'flow-predefined': { const k = Math.min(14, w * 0.1); return P([[0, 0], [w, 0], [w, h], [0, h]]) + `M${k} 0V${h}M${w - k} 0V${h}` }
      default: return P([[0, 0], [w, 0], [w, h], [0, h]])
    }
  }
  function poly(n, w, h, start) {
    const pts = []
    for (let i = 0; i < n; i++) { const a = (start + (i * 360) / n) * Math.PI / 180; pts.push([w / 2 + Math.cos(a) * w / 2, h * 0.55 + Math.sin(a) * h * 0.55]) }
    return pts
  }
  // Where text sits inside a shape (as fractions of the box).
  function textBox(kind) {
    switch (kind) {
      case 'triangle': return [0.22, 0.45, 0.56, 0.5]
      case 'diamond': case 'flow-decision': return [0.2, 0.22, 0.6, 0.56]
      case 'ellipse': case 'cloud': return [0.15, 0.18, 0.7, 0.64]
      case 'star': return [0.33, 0.38, 0.34, 0.4]   // the star's body, inside its arms
      case 'pentagon': return [0.25, 0.3, 0.5, 0.5]
      case 'speech': return [0.06, 0.06, 0.88, 0.66]
      case 'cylinder': return [0.06, 0.2, 0.88, 0.72]
      case 'parallelogram': case 'flow-data': case 'trapezoid': return [0.18, 0.08, 0.64, 0.84]
      case 'arrow-right': case 'arrow-left': return [0.12, 0.25, 0.7, 0.5]
      case 'chevron': return [0.31, 0.25, 0.4, 0.5]   // past the notch on the left (up to 30% of the width)
      default: return [0.06, 0.06, 0.88, 0.88]
    }
  }

  // ---------- connectors ----------
  const SIDES = { top: [0, -1], right: [1, 0], bottom: [0, 1], left: [-1, 0] }
  function anchorPoint(o, side) {
    const cx = o.x + o.w / 2; const cy = o.y + o.h / 2
    if (side === 'top') return [cx, o.y]
    if (side === 'bottom') return [cx, o.y + o.h]
    if (side === 'left') return [o.x, cy]
    return [o.x + o.w, cy]
  }
  function autoSide(o, toward) {
    const cx = o.x + o.w / 2; const cy = o.y + o.h / 2
    const dx = (toward[0] - cx) / Math.max(o.w, 1); const dy = (toward[1] - cy) / Math.max(o.h, 1)
    return Math.abs(dx) > Math.abs(dy) ? (dx > 0 ? 'right' : 'left') : (dy > 0 ? 'bottom' : 'top')
  }
  // Resolve both ends to {p:[x,y], side|null}. An end that names an object that isn't on the board (an agent
  // deleted it without its lines) is "missing": it falls back to its stored point when it has one, and is
  // "unplaced" when it has none. Nothing is ever drawn to (0, 0) by default.
  const hasPoint = (e) => !!e && Number.isFinite(e.x) && Number.isFinite(e.y)
  function ends(c, get) {
    const A = c.from && c.from.id ? get(c.from.id) : null
    const B = c.to && c.to.id ? get(c.to.id) : null
    const missing = { a: !!(c.from && c.from.id && !A), b: !!(c.to && c.to.id && !B) }
    const unplaced = (!A && !hasPoint(c.from)) || (!B && !hasPoint(c.to))
    const centre = (o) => [o.x + o.w / 2, o.y + o.h / 2]
    const pa = A ? centre(A) : hasPoint(c.from) ? [c.from.x, c.from.y] : B ? centre(B) : [0, 0]
    const pb = B ? centre(B) : hasPoint(c.to) ? [c.to.x, c.to.y] : pa
    const sa = A ? (c.from.anchor && c.from.anchor !== 'auto' ? c.from.anchor : autoSide(A, pb)) : null
    const sb = B ? (c.to.anchor && c.to.anchor !== 'auto' ? c.to.anchor : autoSide(B, pa)) : null
    let qa = A ? anchorPoint(A, sa) : pa; let qb = B ? anchorPoint(B, sb) : pb
    // Facing sides of boxes that overlap: run straight through the middle of the overlap.
    const auto = (e) => !e.anchor || e.anchor === 'auto'
    if (A && B && auto(c.from) && auto(c.to)) {
      const vert = (sa === 'top' || sa === 'bottom') && (sb === 'top' || sb === 'bottom') && sa !== sb
      const horiz = (sa === 'left' || sa === 'right') && (sb === 'left' || sb === 'right') && sa !== sb
      if (vert) { const l = Math.max(A.x, B.x); const r = Math.min(A.x + A.w, B.x + B.w); if (r - l > 8) { const m = (l + r) / 2; qa = [m, qa[1]]; qb = [m, qb[1]] } }
      if (horiz) { const t = Math.max(A.y, B.y); const bt = Math.min(A.y + A.h, B.y + B.h); if (bt - t > 8) { const m = (t + bt) / 2; qa = [qa[0], m]; qb = [qb[0], m] } }
    }
    return { a: { p: qa, side: sa }, b: { p: qb, side: sb }, missing: missing.a || missing.b, unplaced }
  }

  // Route → {d: svg path, pts: polyline for hit/label, start, end, missing, unplaced}
  function route(c, get) {
    const e = ends(c, get)
    const r = route0(c, e)
    r.missing = e.missing; r.unplaced = e.unplaced
    return r
  }
  function route0(c, { a, b }) {
    const kind = c.route || 'elbow'
    const pa = a.p; const pb = b.p
    if (kind === 'straight') {
      return { d: `M${f(pa[0])} ${f(pa[1])}L${f(pb[0])} ${f(pb[1])}`, pts: [pa, pb], start: dir(pb, pa), end: dir(pa, pb) }
    }
    const da = a.side ? SIDES[a.side] : unit(pa, pb); const db = b.side ? SIDES[b.side] : unit(pb, pa)
    if (kind === 'curved') {
      const dist = Math.hypot(pb[0] - pa[0], pb[1] - pa[1]); const k = Math.max(40, dist * 0.4)
      const c1 = [pa[0] + da[0] * k, pa[1] + da[1] * k]; const c2 = [pb[0] + db[0] * k, pb[1] + db[1] * k]
      const pts = []; for (let i = 0; i <= 16; i++) pts.push(bez(pa, c1, c2, pb, i / 16))
      return { d: `M${f(pa[0])} ${f(pa[1])}C${f(c1[0])} ${f(c1[1])} ${f(c2[0])} ${f(c2[1])} ${f(pb[0])} ${f(pb[1])}`, pts, start: dir(c1, pa), end: dir(c2, pb) }
    }
    const pts = elbow(pa, da, pb, db)
    return { d: rounded(pts, 10), pts, start: dir(pts[1], pts[0]), end: dir(pts[pts.length - 2], pts[pts.length - 1]) }
  }
  function unit(p, q) { const dx = q[0] - p[0]; const dy = q[1] - p[1]; return Math.abs(dx) > Math.abs(dy) ? [Math.sign(dx) || 1, 0] : [0, Math.sign(dy) || 1] }
  function dir(from, to) { const dx = to[0] - from[0]; const dy = to[1] - from[1]; const l = Math.hypot(dx, dy) || 1; return [dx / l, dy / l] }
  function bez(p0, p1, p2, p3, t) {
    const u = 1 - t
    return [u * u * u * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t * t * t * p3[0], u * u * u * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t * t * t * p3[1]]
  }
  // Orthogonal route leaving A along da and entering B against db.
  function elbow(pa, da, pb, db) {
    const G = 24
    const a1 = [pa[0] + da[0] * G, pa[1] + da[1] * G]; const b1 = [pb[0] + db[0] * G, pb[1] + db[1] * G]
    const pts = [pa, a1]
    const horizA = da[0] !== 0; const horizB = db[0] !== 0
    if (horizA && horizB) { const mx = (a1[0] + b1[0]) / 2; pts.push([mx, a1[1]], [mx, b1[1]]) }
    else if (!horizA && !horizB) { const my = (a1[1] + b1[1]) / 2; pts.push([a1[0], my], [b1[0], my]) }
    else if (horizA) pts.push([b1[0], a1[1]])
    else pts.push([a1[0], b1[1]])
    pts.push(b1, pb)
    // Drop repeated and collinear points.
    const out = [pts[0]]
    for (let i = 1; i < pts.length; i++) {
      const p = pts[i]; const q = out[out.length - 1]
      if (Math.abs(p[0] - q[0]) < 0.5 && Math.abs(p[1] - q[1]) < 0.5) continue
      if (out.length >= 2) { const r = out[out.length - 2]; if ((Math.abs(r[0] - q[0]) < 0.5 && Math.abs(q[0] - p[0]) < 0.5) || (Math.abs(r[1] - q[1]) < 0.5 && Math.abs(q[1] - p[1]) < 0.5)) { out[out.length - 1] = p; continue } }
      out.push(p)
    }
    return out
  }
  function rounded(pts, r) {
    let d = `M${f(pts[0][0])} ${f(pts[0][1])}`
    for (let i = 1; i < pts.length - 1; i++) {
      const p = pts[i - 1]; const q = pts[i]; const n = pts[i + 1]
      const l1 = Math.hypot(q[0] - p[0], q[1] - p[1]); const l2 = Math.hypot(n[0] - q[0], n[1] - q[1])
      const k = Math.min(r, l1 / 2, l2 / 2)
      const s = [q[0] + (p[0] - q[0]) * k / l1, q[1] + (p[1] - q[1]) * k / l1]; const e = [q[0] + (n[0] - q[0]) * k / l2, q[1] + (n[1] - q[1]) * k / l2]
      d += `L${f(s[0])} ${f(s[1])}Q${f(q[0])} ${f(q[1])} ${f(e[0])} ${f(e[1])}`
    }
    const z = pts[pts.length - 1]
    return d + `L${f(z[0])} ${f(z[1])}`
  }
  // The point halfway along a polyline (for labels).
  function midpoint(pts) {
    let total = 0; for (let i = 1; i < pts.length; i++) total += Math.hypot(pts[i][0] - pts[i - 1][0], pts[i][1] - pts[i - 1][1])
    let half = total / 2
    for (let i = 1; i < pts.length; i++) {
      const l = Math.hypot(pts[i][0] - pts[i - 1][0], pts[i][1] - pts[i - 1][1])
      if (half <= l) { const t = l ? half / l : 0; return [pts[i - 1][0] + (pts[i][0] - pts[i - 1][0]) * t, pts[i - 1][1] + (pts[i][1] - pts[i - 1][1]) * t] }
      half -= l
    }
    return pts[0]
  }
  // Arrowhead at tip p, pointing along unit vector d. Returns {d, filled}.
  function cap(kind, p, d, sw) {
    const s = 6 + sw * 2.2
    const nx = -d[1]; const ny = d[0]
    const back = (k) => [p[0] - d[0] * s * k, p[1] - d[1] * s * k]
    const b = back(1)
    if (kind === 'arrow') return { d: `M${f(b[0] + nx * s * 0.6)} ${f(b[1] + ny * s * 0.6)}L${f(p[0])} ${f(p[1])}L${f(b[0] - nx * s * 0.6)} ${f(b[1] - ny * s * 0.6)}`, filled: false }
    if (kind === 'triangle') return { d: `M${f(p[0])} ${f(p[1])}L${f(b[0] + nx * s * 0.55)} ${f(b[1] + ny * s * 0.55)}L${f(b[0] - nx * s * 0.55)} ${f(b[1] - ny * s * 0.55)}Z`, filled: true }
    if (kind === 'circle') { const c = back(0.5); const rr = s * 0.4; return { d: `M${f(c[0] - rr)} ${f(c[1])}a${f(rr)} ${f(rr)} 0 1 0 ${f(rr * 2)} 0a${f(rr)} ${f(rr)} 0 1 0 ${f(-rr * 2)} 0Z`, filled: true } }
    if (kind === 'diamond') { const m = back(0.6); const e = back(1.2); return { d: `M${f(p[0])} ${f(p[1])}L${f(m[0] + nx * s * 0.45)} ${f(m[1] + ny * s * 0.45)}L${f(e[0])} ${f(e[1])}L${f(m[0] - nx * s * 0.45)} ${f(m[1] - ny * s * 0.45)}Z`, filled: true } }
    if (kind === 'bar') return { d: `M${f(p[0] + nx * s * 0.6)} ${f(p[1] + ny * s * 0.6)}L${f(p[0] - nx * s * 0.6)} ${f(p[1] - ny * s * 0.6)}`, filled: false }
    return null
  }

  // ---------- pen ----------
  // Smooth a stroke through midpoints of its points.
  function penPath(pts) {
    if (!pts.length) return ''
    if (pts.length < 3) return `M${pts[0][0]} ${pts[0][1]}` + pts.slice(1).map((p) => `L${p[0]} ${p[1]}`).join('') + (pts.length === 1 ? 'l0.1 0' : '')
    let d = `M${pts[0][0]} ${pts[0][1]}`
    for (let i = 1; i < pts.length - 1; i++) {
      const m = [(pts[i][0] + pts[i + 1][0]) / 2, (pts[i][1] + pts[i + 1][1]) / 2]
      d += `Q${pts[i][0]} ${pts[i][1]} ${f(m[0])} ${f(m[1])}`
    }
    const z = pts[pts.length - 1]
    return d + `L${z[0]} ${z[1]}`
  }
  // Drop points closer than tol to the line (Ramer–Douglas–Peucker).
  function simplify(pts, tol) {
    if (pts.length < 3) return pts
    const keep = new Array(pts.length).fill(false); keep[0] = keep[pts.length - 1] = true
    const stack = [[0, pts.length - 1]]
    while (stack.length) {
      const [s, e] = stack.pop(); let maxD = 0; let idx = -1
      const [x1, y1] = pts[s]; const [x2, y2] = pts[e]; const L = Math.hypot(x2 - x1, y2 - y1) || 1
      for (let i = s + 1; i < e; i++) { const d = Math.abs((y2 - y1) * pts[i][0] - (x2 - x1) * pts[i][1] + x2 * y1 - y2 * x1) / L; if (d > maxD) { maxD = d; idx = i } }
      if (maxD > tol) { keep[idx] = true; stack.push([s, idx], [idx, e]) }
    }
    return pts.filter((_, i) => keep[i])
  }

  // ---------- boxes ----------
  function bounds(list) {
    if (!list.length) return null
    let x1 = Infinity; let y1 = Infinity; let x2 = -Infinity; let y2 = -Infinity
    for (const b of list) { x1 = Math.min(x1, b.x); y1 = Math.min(y1, b.y); x2 = Math.max(x2, b.x + b.w); y2 = Math.max(y2, b.y + b.h) }
    return { x: x1, y: y1, w: x2 - x1, h: y2 - y1 }
  }
  const intersects = (a, b) => a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
  const inside = (a, b) => a.x >= b.x && a.y >= b.y && a.x + a.w <= b.x + b.w && a.y + a.h <= b.y + b.h
  function distToPolyline(p, pts) {
    let best = Infinity
    for (let i = 1; i < pts.length; i++) {
      const a = pts[i - 1]; const b = pts[i]; const dx = b[0] - a[0]; const dy = b[1] - a[1]; const l2 = dx * dx + dy * dy
      const t = l2 ? Math.max(0, Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2)) : 0
      best = Math.min(best, Math.hypot(p[0] - a[0] - t * dx, p[1] - a[1] - t * dy))
    }
    return best
  }

  g.BoardGeom = { shapePath, textBox, route, ends, midpoint, cap, penPath, simplify, bounds, intersects, inside, distToPolyline, anchorPoint }
})(window)
