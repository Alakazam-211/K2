// The overview: on a long edit only, the whole timeline in one 14 px strip
// under the lanes: every clip as a sliver in its lane's colour, your screen
// as a box, everyone's playhead as a line. A click there brings the lanes
// to that place (the lanes themselves scroll by keyboard).

import { useEffect, useRef, type CSSProperties } from 'react'
import { lanesOf, type TimelineModel } from '@/lib/live-view/timeline/fold'
import type { Playhead } from '@/lib/live-view/timeline/lanes-view'
import type { Box } from './box'
import { PAINTS } from './measure'

export function Overview({ model, v0, v1, heads, onGo }: { model: Box<TimelineModel>; v0: number; v1: number; heads: Playhead[]; onGo: (f: number) => void }): React.JSX.Element {
  const M = model.v
  const box = useRef<HTMLDivElement>(null)
  const cv = useRef<HTMLCanvasElement>(null)
  const span = Math.max(48, M.endF * 1.03)
  const W = box.current?.clientWidth ?? 0
  const sx = W ? W / span : 0
  useEffect(() => {
    const c = cv.current
    const b = box.current
    if (!PAINTS || !c || !b) return
    const w = b.clientWidth
    const h = b.clientHeight
    if (w < 10) return
    c.width = w * 2
    c.height = h * 2
    const g = c.getContext('2d')
    if (!g) return
    g.scale(2, 2)
    g.clearRect(0, 0, w, h)
    const s = w / span
    const ink = (cls: string): string => getComputedStyle(b.querySelector(`.${cls}`) as Element).backgroundColor
    const col = { v: ink('lv-tl-ov-v'), a: ink('lv-tl-ov-a'), g: ink('lv-tl-ov-g') }
    const L = lanesOf(M)
    const rh = Math.max(1, (h - 2) / Math.max(1, L.length))
    L.forEach((t, i) => {
      const y = 1 + i * rh
      for (const it of t.items) {
        if (it.kind === 'transition') continue
        g.globalAlpha = it.off ? 0.35 : 1
        g.fillStyle = it.kind === 'generator' ? col.g : t.kind === 'video' ? col.v : col.a
        g.fillRect(it.f0 * s, y, Math.max(0.6, (it.f1 - it.f0) * s - 0.4), Math.max(1, rh - 0.6))
      }
    })
  }, [M, span])
  return (
    <div ref={box} className="lv-tl-ovw" data-testid="lv-overview" aria-hidden="true">
      <i className="lv-tl-ov-v" hidden />
      <i className="lv-tl-ov-a" hidden />
      <i className="lv-tl-ov-g" hidden />
      <canvas
        ref={cv}
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect()
          if (r.width > 0) onGo(((e.clientX - r.left) / r.width) * span)
        }}
      />
      {sx ? <i className="lv-tl-ovv" style={{ left: v0 * sx, width: Math.max(3, (v1 - v0) * sx) }} /> : null}
      {sx
        ? heads.map((h) => <i key={h.id} className="lv-tl-ovh" style={{ left: h.f * sx, '--lv-c': h.self ? 'var(--color-accent)' : h.colour } as CSSProperties} />)
        : null}
    </div>
  )
}
