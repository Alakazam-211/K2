// The sample's footage: each source drawn in the page from its `_footage`
// (the mock's stand-in for real video: interviews, city, sea, harbour,
// forest, road), changing slowly with source time. Sample only (it ships in
// the sample's lazy chunk); the lanes' pictures are these stills until K2
// Render's tiles (T8), and the monitor's sample footage (T4) draws the same.
// No timecode is burned in (v2.3). Also the sample waveforms' peaks.

import type { MediaInfo } from './picture'

interface Footage {
  scene: string
  pal: string[]
  seed: number
}

const hx = (c: string): number[] => {
  const n = parseInt(String(c).slice(1), 16)
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255]
}
const rgba = (c: string, a: number): string => `rgba(${hx(c).join(',')},${a})`
const mix = (a: string, b: string, t: number): string => {
  const A = hx(a)
  const B = hx(b)
  return '#' + A.map((v, i) => Math.round(v + (B[i] - v) * t).toString(16).padStart(2, '0')).join('')
}
const nz = (i: number, s: number): number => {
  const x = Math.sin(i * 12.9898 + (s || 0) * 78.233) * 43758.5453
  return x - Math.floor(x)
}
function mulberry(a: number): () => number {
  return () => {
    a |= 0
    a = (a + 0x6d2b79f5) | 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const DEFAULT: Footage = { scene: 'city', pal: ['#30364a', '#58607a', '#20242f', '#ffd98a'], seed: 7 }
const footageOf = (mi: MediaInfo | null | undefined): Footage => ((mi as { _footage?: Footage } | null)?._footage as Footage) || DEFAULT

/** Draw a source's picture at source frame `sf` into a w × h context. */
export function drawScene(g: CanvasRenderingContext2D, w: number, h: number, mi: MediaInfo | null | undefined, sf: number): void {
  const fo = footageOf(mi)
  const C = fo.pal || ['#333333', '#666666', '#222222', '#ffffff']
  const t = (sf || 0) / 24
  const R = mulberry(fo.seed * 131 + 7)
  const lg = (y0: number, y1: number, a: string, b: string): CanvasGradient => {
    const q = g.createLinearGradient(0, y0, 0, y1)
    q.addColorStop(0, a)
    q.addColorStop(1, b)
    return q
  }
  const ell = (x: number, y: number, rx: number, ry: number, fill: string): void => {
    g.beginPath()
    g.ellipse(x, y, Math.max(0.1, rx), Math.max(0.1, ry), 0, 0, Math.PI * 2)
    g.fillStyle = fill
    g.fill()
  }
  const poly = (pts: number[][], fill: string): void => {
    g.beginPath()
    pts.forEach(([x, y], i) => (i ? g.lineTo(x, y) : g.moveTo(x, y)))
    g.closePath()
    g.fillStyle = fill
    g.fill()
  }
  g.save()
  const sc = fo.scene
  if (sc === 'interview') {
    g.fillStyle = lg(0, h, mix(C[0], '#ffffff', 0.14), C[0])
    g.fillRect(0, 0, w, h)
    g.fillStyle = rgba(C[1], 0.6)
    g.fillRect(w * 0.6, h * 0.1, w * 0.28, h * 0.46)
    g.strokeStyle = mix(C[0], '#000000', 0.25)
    g.lineWidth = Math.max(1, w * 0.007)
    g.strokeRect(w * 0.6, h * 0.1, w * 0.28, h * 0.46)
    g.beginPath()
    g.moveTo(w * 0.74, h * 0.1)
    g.lineTo(w * 0.74, h * 0.56)
    g.moveTo(w * 0.6, h * 0.33)
    g.lineTo(w * 0.88, h * 0.33)
    g.stroke()
    g.fillStyle = mix(C[0], '#000000', 0.35)
    g.fillRect(w * 0.04, h * 0.62, w * 0.16, h * 0.4)
    ;[[0.08, 0.55], [0.12, 0.5], [0.16, 0.56], [0.1, 0.46]].forEach(([x, y], k) => ell(w * x, h * y, w * 0.035, h * 0.07, mix('#2f5a34', C[0], 0.25 + k * 0.08)))
    const cx = w * 0.42 + Math.sin(t * 0.8 + fo.seed) * w * 0.007
    const nod = Math.sin(t * 1.6 + fo.seed * 2) * h * 0.006
    g.fillStyle = C[2]
    g.beginPath()
    g.moveTo(cx - w * 0.2, h * 1.02)
    g.quadraticCurveTo(cx - w * 0.2, h * 0.66, cx, h * 0.64)
    g.quadraticCurveTo(cx + w * 0.2, h * 0.66, cx + w * 0.2, h * 1.02)
    g.fill()
    g.fillStyle = mix(C[3], '#000000', 0.12)
    g.fillRect(cx - w * 0.028, h * 0.5 + nod, w * 0.056, h * 0.16)
    ell(cx, h * 0.42 + nod, w * 0.066, h * 0.135, C[3])
    g.fillStyle = mix(C[0], '#000000', 0.55)
    g.beginPath()
    g.ellipse(cx, h * 0.36 + nod, w * 0.07, h * 0.09, 0, Math.PI, Math.PI * 2)
    g.fill()
  } else if (sc === 'city') {
    g.fillStyle = lg(0, h * 0.8, C[0], C[1])
    g.fillRect(0, 0, w, h)
    ell(w * 0.72, h * 0.32, h * 0.09, h * 0.09, rgba(C[3], 0.9))
    let x = -w * 0.02
    while (x < w) {
      const bw = w * (0.04 + R() * 0.06)
      const bh = h * (0.22 + R() * 0.42)
      g.fillStyle = mix(C[2], '#000000', R() * 0.3)
      g.fillRect(x, h * 0.92 - bh, bw, bh)
      for (let yy = h * 0.92 - bh + h * 0.03; yy < h * 0.9; yy += h * 0.045)
        for (let xx = x + bw * 0.15; xx < x + bw * 0.85; xx += bw * 0.22)
          if (R() < 0.35) {
            g.fillStyle = rgba(C[3], 0.85)
            g.fillRect(xx, yy, bw * 0.1, h * 0.018)
          }
      x += bw + w * 0.004
    }
    g.fillStyle = mix(C[2], '#000000', 0.4)
    g.fillRect(0, h * 0.92, w, h * 0.08)
  } else if (sc === 'sea' || sc === 'harbour') {
    const hz = sc === 'sea' ? 0.5 : 0.62
    g.fillStyle = lg(0, h * hz, C[0], mix(C[0], C[3], 0.45))
    g.fillRect(0, 0, w, h * hz)
    ell(w * 0.3, h * (hz - 0.2), h * 0.07, h * 0.07, rgba(C[3], 0.95))
    g.fillStyle = lg(h * hz, h, C[1], C[2])
    g.fillRect(0, h * hz, w, h * (1 - hz))
    for (let k = 0; k < 10; k++) {
      const y = h * (hz + 0.02 + (k * (1 - hz)) / 10)
      g.strokeStyle = `rgba(255,255,255,${0.1 + k * 0.012})`
      g.lineWidth = Math.max(1, h * 0.003)
      g.beginPath()
      for (let xx = 0; xx <= w; xx += w / 40) {
        const yy = y + Math.sin((xx / (w * (0.12 + 0.02 * k))) * 6.28 + t * (0.6 + k * 0.1)) * h * 0.005 * (1 + k * 0.3)
        if (xx) g.lineTo(xx, yy)
        else g.moveTo(xx, yy)
      }
      g.stroke()
    }
    if (sc === 'harbour') {
      const dk = mix(C[2], '#000000', 0.55)
      g.strokeStyle = dk
      g.lineWidth = Math.max(1, w * 0.006)
      ;[[0.62, 0.3], [0.74, 0.24], [0.86, 0.33]].forEach(([x0, top], k) => {
        g.beginPath()
        g.moveTo(w * x0, h * hz)
        g.lineTo(w * x0, h * top)
        g.lineTo(w * (x0 - 0.12 + k * 0.02), h * (top + 0.02))
        g.stroke()
      })
      g.fillStyle = dk
      g.fillRect(w * 0.55, h * (hz - 0.05), w * 0.45, h * 0.05)
    }
    const bx = w * (0.12 + ((t * 0.012 + fo.seed * 0.001) % 0.7))
    const by = h * (hz + 0.06)
    poly([[bx - w * 0.04, by], [bx + w * 0.04, by], [bx + w * 0.03, by + h * 0.025], [bx - w * 0.03, by + h * 0.025]], mix(C[2], '#000000', 0.5))
    poly([[bx, by - h * 0.12], [bx, by - h * 0.005], [bx + w * 0.035, by - h * 0.005]], rgba('#ffffff', 0.9))
  } else if (sc === 'forest') {
    g.fillStyle = lg(0, h * 0.7, C[0], mix(C[0], '#ffffff', 0.3))
    g.fillRect(0, 0, w, h)
    ell(w * 0.7, h * 0.25, h * 0.08, h * 0.08, rgba(C[3], 0.95))
    ;([[0.5, mix(C[1], C[0], 0.55)], [0.62, C[1]], [0.76, C[2]]] as [number, string][]).forEach(([y0, col], k) => {
      g.fillStyle = col
      g.beginPath()
      g.moveTo(0, h)
      for (let xx = 0; xx <= w; xx += w / 30) g.lineTo(xx, h * y0 + Math.sin((xx / w) * 5 + k * 2) * h * 0.04)
      g.lineTo(w, h)
      g.fill()
      for (let xx = -w * 0.08; xx < w * 1.08; xx += w * 0.08) {
        const tt = h * (0.1 + 0.04 * k) * (0.7 + nz(Math.round(xx / (w * 0.08)) + k * 50, fo.seed) * 0.6)
        const yb = h * y0 + Math.sin((xx / w) * 5 + k * 2) * h * 0.04 + h * 0.02
        poly([[xx, yb - tt], [xx - tt * 0.28, yb], [xx + tt * 0.28, yb]], mix(col, '#000000', 0.18))
      }
    })
  } else if (sc === 'road') {
    g.fillStyle = lg(0, h * 0.5, C[0], mix(C[0], '#ffffff', 0.5))
    g.fillRect(0, 0, w, h * 0.5)
    g.fillStyle = C[1]
    g.fillRect(0, h * 0.5, w, h * 0.5)
    poly([[w * 0.47, h * 0.5], [w * 0.53, h * 0.5], [w * 0.95, h], [w * 0.05, h]], C[2])
    for (let k = 0; k < 9; k++) {
      const z = ((k + t * 1.4) % 9) / 9
      const z2 = Math.min(1, z + 0.045)
      const y1 = h * 0.5 + h * 0.5 * z * z
      const y2 = h * 0.5 + h * 0.5 * z2 * z2
      const a = w * 0.004 * (0.2 + z * 3)
      const b = w * 0.004 * (0.2 + z2 * 3)
      poly([[w * 0.5 - a, y1], [w * 0.5 + a, y1], [w * 0.5 + b, y2], [w * 0.5 - b, y2]], rgba(C[3], 0.9))
    }
  } else {
    g.fillStyle = lg(0, h, C[0], C[2])
    g.fillRect(0, 0, w, h)
    for (let k = 0; k < 6; k++) ell(w * nz(k, fo.seed), h * nz(k + 9, fo.seed), w * 0.08, w * 0.08, rgba(C[3], 0.2))
  }
  g.restore()
}

/** A sample waveform's height (0..1) at source frame `sf`: speech comes in words and phrases, music on its beat. */
export function waveAmp(mi: MediaInfo | null | undefined, sf: number): number {
  const seed = footageOf(mi).seed || 3
  if (mi && mi.kind === 'audio') {
    const beat = Math.pow(Math.max(0, Math.cos(((sf % 12) / 12) * Math.PI * 2)), 6)
    return 0.35 + 0.18 * Math.sin(sf / 37) + 0.3 * beat + 0.12 * nz(sf, seed)
  }
  const word = nz(Math.floor(sf / 5), seed) > 0.32 ? 1 : 0.12
  const phrase = nz(Math.floor(sf / 44), seed + 3) > 0.22 ? 1 : 0.08
  return Math.min(1, word * phrase * (0.35 + 0.65 * nz(sf, seed + 1)))
}

const TILES = new Map<string, HTMLCanvasElement>()

/** A tile drawn once per (source, source frame, size) and kept, as K2 Render's cache would be. */
export function tileCanvas(mi: MediaInfo, sf: number, w = 160, h = 90): HTMLCanvasElement | null {
  const key = `${mi.hash}|${sf}|${w}`
  const hit = TILES.get(key)
  if (hit) return hit
  const c = document.createElement('canvas')
  c.width = w
  c.height = h
  const g = c.getContext('2d')
  if (!g) return null
  drawScene(g, w, h, mi, sf)
  if (TILES.size > 1500) TILES.clear()
  TILES.set(key, c)
  return c
}
