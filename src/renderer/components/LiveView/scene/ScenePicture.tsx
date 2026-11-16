// The still on the stage: letterboxed in its pane, the transparent still
// over the dark stage, its edges fading into it. A new still fades in over
// the one before (the old one stays up until it lands). With no picture,
// the ground grid and a dot per object's origin. Overlays (boxes, tags)
// sit in the same frame from S4.

import { useEffect, useLayoutEffect, useMemo, useRef } from 'react'
import { groundOf } from '@/lib/live-view/scene/ground'
import { STILL_WIDTH, stillHeight, type Letterbox, type Still } from '@/lib/live-view/scene/picture-source'
import type { Doc } from '@/lib/live-view/scene/types'

function paint(cv: HTMLCanvasElement, st: Still): void {
  if (cv.width !== st.width) cv.width = st.width
  if (cv.height !== st.height) cv.height = st.height
  const ctx = cv.getContext('2d')
  if (!ctx) return
  ctx.clearRect(0, 0, st.width, st.height)
  if (st.image.kind === 'bitmap') ctx.drawImage(st.image.image, 0, 0, st.width, st.height)
  else if (typeof ImageData !== 'undefined') ctx.putImageData(new ImageData(st.image.data as Uint8ClampedArray<ArrayBuffer>, st.width, st.height), 0, 0)
}

export function ScenePicture({ lb, still, label, dim, ground, motion }: { lb: Letterbox; still: Still | null; label: string; dim: boolean; ground: Doc | null; motion: boolean }): React.JSX.Element {
  const cur = useRef<HTMLCanvasElement>(null)
  const old = useRef<HTMLCanvasElement>(null)
  const shown = useRef<Still | null>(null)
  useLayoutEffect(() => {
    const cv = cur.current
    if (!cv || !still || still === shown.current) return
    const prev = shown.current
    const o = old.current
    // The old still fades out over the new one (not on the first, and not without motion).
    if (prev && o && motion && (prev.view !== still.view || prev.rev !== still.rev)) {
      o.width = cv.width
      o.height = cv.height
      o.getContext('2d')?.drawImage(cv, 0, 0)
      o.setAttribute('data-fading', '')
      void o.offsetWidth
      o.removeAttribute('data-fading')
    }
    paint(cv, still)
    shown.current = still
  }, [still, motion])
  useEffect(() => {
    if (!still) shown.current = null
  }, [still])

  const g = useMemo(() => (ground ? { W: STILL_WIDTH, H: stillHeight(STILL_WIDTH, lb.shape), ...groundOf(ground, STILL_WIDTH, stillHeight(STILL_WIDTH, lb.shape)) } : null), [ground, lb.shape])
  return (
    <div className="lv-frame" data-cover={lb.cover ? '' : undefined} data-dim={dim ? '' : undefined} data-rev={still?.rev} data-view={still?.view} style={{ left: lb.left, top: lb.top, width: lb.width, height: lb.height }}>
      {g ? (
        <svg data-testid="lv-scene-ground" className="lv-ground" viewBox={`0 0 ${g.W} ${g.H}`} preserveAspectRatio="none" aria-hidden="true">
          {g.lines.map((l, i) => (
            <line key={i} x1={l.x1} y1={l.y1} x2={l.x2} y2={l.y2} />
          ))}
          {g.dots.map((d) => (
            <circle key={d.id} cx={d.x} cy={d.y} r={6} />
          ))}
        </svg>
      ) : null}
      <canvas ref={old} className="lv-old" aria-hidden="true" hidden={!still} />
      <canvas ref={cur} role="img" aria-label={label} hidden={!still} />
    </div>
  )
}
