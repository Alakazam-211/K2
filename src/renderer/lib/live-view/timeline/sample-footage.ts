// The sample's monitor footage: the mock's drawn sources (footage.ts) as a
// MonitorSource. A frame is drawn at the monitor's size; a tile is the
// lanes' 160 px tile scaled up (a still looks like one); the poster is
// drawn once per source at 640 px; seeking draws a small picture scaled
// up, so it looks soft. Sample only: it ships in the sample's lazy chunk
// and the monitor shows it only for the sample (monitorSourceFor).

import { drawScene, tileCanvas } from './footage'
import { NO_FOOTAGE, type MonitorPaint, type MonitorSource } from './monitor-source'
import type { MediaInfo } from './picture'

const POSTERS = new Map<string, HTMLCanvasElement>()
let soft: HTMLCanvasElement | null = null

/** The source's poster (640 px), a little into the clip, drawn once. */
function posterCanvas(mi: MediaInfo, inF: number): HTMLCanvasElement | null {
  const key = `${mi.hash}|${inF}`
  const hit = POSTERS.get(key)
  if (hit) return hit
  const c = document.createElement('canvas')
  c.width = 640
  c.height = 360
  const g = c.getContext('2d')
  if (!g) return null
  drawScene(g, 640, 360, mi, inF + 36)
  if (POSTERS.size > 200) POSTERS.clear()
  POSTERS.set(key, c)
  return c
}

function blit(g: CanvasRenderingContext2D, w: number, h: number, c: HTMLCanvasElement | null): void {
  if (!c) return
  g.imageSmoothingEnabled = true
  g.imageSmoothingQuality = 'high'
  g.drawImage(c, 0, 0, w, h)
}

export const SampleFootage: MonitorSource = {
  kind: 'sample',
  sample: true,
  paint(g: CanvasRenderingContext2D, w: number, h: number, p: MonitorPaint): void {
    g.fillStyle = '#000'
    g.fillRect(0, 0, w, h)
    const mi = p.mi
    if (!mi || p.how === 'none') return
    if (p.how === 'frame') {
      drawScene(g, w, h, mi, p.srcF ?? 0)
      return
    }
    if (p.how === 'tile') {
      blit(g, w, h, tileCanvas(mi, p.srcF ?? 0, 160, 90))
      return
    }
    if (p.how === 'soft') {
      const lw = Math.max(16, Math.round(w / 5.5))
      const lh = Math.round((lw * 9) / 16)
      soft ??= document.createElement('canvas')
      soft.width = lw
      soft.height = lh
      const sg = soft.getContext('2d')
      if (!sg) return
      drawScene(sg, lw, lh, mi, p.srcF ?? 0)
      blit(g, w, h, soft)
      return
    }
    blit(g, w, h, posterCanvas(mi, p.inF ?? 0))
    if (p.how === 'dim') {
      g.fillStyle = 'rgba(0,0,0,.4)'
      g.fillRect(0, 0, w, h)
    }
  },
}

/** The monitor's source for a view: the sample's footage only for the sample; nothing else ever draws it. */
export const monitorSourceFor = (sample: boolean): MonitorSource => (sample ? SampleFootage : NO_FOOTAGE)
