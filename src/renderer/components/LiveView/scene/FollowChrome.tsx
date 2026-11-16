// What Follow, two views and the turntable put on the stage (S5):
//   - chips by the picker for who looks through another view ("Brook ·
//     Camera 2"); a chip follows that person (up to three, then +N);
//   - the finished line once following ends ("Atlas stopped. Stopped
//     following." with Back to my place and Follow again);
//   - the two-views switch in the stage's foot;
//   - the turntable strip at the picture's foot: eight stills, click or
//     drag across to turn, arrow keys too. It says they're stills.

import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui'
import { turntableFrameAt, TURNTABLE_FRAMES, type OtherView } from '@/lib/live-view/scene/follow-view'
import { THUMB_WIDTH, type PictureSource, type Still } from '@/lib/live-view/scene/picture-source'
import { turntableView } from '@/lib/live-view/scene/view-address'
import { LiveAvatar } from '../shell/LiveAvatar'
import { TwoSideGlyph, TwoStackGlyph } from './scene-glyphs'

const MAX_CHIPS = 3

export function PeerChips({ others, onFollow }: { others: OtherView[]; onFollow: (id: string) => void }): React.JSX.Element | null {
  if (!others.length) return null
  const shown = others.slice(0, MAX_CHIPS)
  const more = others.length - shown.length
  return (
    <>
      {shown.map((o) => {
        const say = `${o.name} is looking through ${o.label}. Follow ${o.name}`
        return (
          <button key={o.id} type="button" className="lv-vpeer lv-plate no-drag" data-k2-focus="" data-k2-chamfer="chip" data-away={o.away ? '' : undefined} aria-label={say} title={say} onClick={() => onFollow(o.id)}>
            <LiveAvatar person={{ name: o.name, kind: o.kind === 'agent' ? 'agent' : 'person', colour: o.colour, away: o.away }} size={16} />
            <span>{o.label}</span>
          </button>
        )
      })}
      {more > 0 ? (
        <span className="lv-vpeer lv-plate" data-k2-chamfer="chip" title={others.slice(MAX_CHIPS).map((o) => `${o.name}: ${o.label}`).join(', ')}>
          +{more}
        </span>
      ) : null}
    </>
  )
}

export function FinishedLine({ words, onBack, onAgain }: { words: { text: string; back: string; again: string | null }; onBack: () => void; onAgain: () => void }): React.JSX.Element {
  return (
    <div className="lv-fslot">
      <div className="lv-fin lv-plate" data-k2-chamfer="surface" role="status" data-testid="lv-finished">
        <span className="lv-fin-t">{words.text}</span>
        <span className="lv-fin-b" role="group" aria-label="After following">
          <Button size="sm" variant="default" data-k2-focus="" data-k2-chamfer="control" onClick={onBack}>
            {words.back}
          </Button>
          {words.again ? (
            <Button size="sm" variant="default" data-k2-focus="" data-k2-chamfer="control" onClick={onAgain}>
              {words.again}
            </Button>
          ) : null}
        </span>
      </div>
    </div>
  )
}

export function TwoViewsButton({ two, stacked, onToggle }: { two: boolean; stacked: boolean; onToggle: () => void }): React.JSX.Element {
  const say = two ? 'Show one view' : 'Show two views'
  return (
    <button type="button" className="lv-twob lv-plate no-drag" data-k2-focus="" data-k2-chamfer="control" aria-pressed={two} aria-label={say} title={say} onClick={onToggle}>
      {stacked ? <TwoStackGlyph size={15} /> : <TwoSideGlyph size={15} />}
    </button>
  )
}

/** A turntable frame's small still. */
function Thumb({ source, rev, k, on }: { source: PictureSource | null; rev: number | null; k: number; on: boolean }): React.JSX.Element {
  const ref = useRef<HTMLCanvasElement>(null)
  const [st, setSt] = useState<Still | null>(null)
  useEffect(() => {
    if (!source || rev == null) return
    const ac = new AbortController()
    void source.still({ rev, view: turntableView(k), shape: 'wide', width: THUMB_WIDTH }, ac.signal).then((s) => {
      if (!ac.signal.aborted) setSt(s)
    })
    return () => ac.abort()
  }, [source, rev, k])
  useEffect(() => {
    const cv = ref.current
    if (!cv || !st) return
    cv.width = st.width
    cv.height = st.height
    const ctx = cv.getContext('2d')
    if (!ctx) return
    if (st.image.kind === 'bitmap') ctx.drawImage(st.image.image, 0, 0)
    else if (typeof ImageData !== 'undefined') ctx.putImageData(new ImageData(st.image.data as Uint8ClampedArray<ArrayBuffer>, st.width, st.height), 0, 0)
  }, [st])
  return (
    <span className="lv-ttf" data-on={on ? '' : undefined}>
      <canvas ref={ref} aria-hidden="true" />
    </span>
  )
}

/** Eight stills round the scene: click or drag across to turn; Left and Right step. */
export function TurntableStrip({ frame, source, rev, onFrame }: { frame: number; source: PictureSource | null; rev: number | null; onFrame: (k: number) => void }): React.JSX.Element {
  const strip = useRef<HTMLDivElement>(null)
  const dragging = useRef(false)
  const at = (clientX: number): number => {
    const r = strip.current?.getBoundingClientRect()
    return r ? turntableFrameAt(clientX - r.left, r.width) : frame
  }
  return (
    <div className="lv-tt lv-plate" data-k2-chamfer="surface" data-testid="lv-turntable">
      <div
        ref={strip}
        className="lv-ttrow"
        role="slider"
        tabIndex={0}
        data-k2-focus=""
        aria-label="Turntable: 8 still pictures round the scene. Drag or use the arrow keys to turn"
        aria-valuemin={1}
        aria-valuemax={TURNTABLE_FRAMES}
        aria-valuenow={frame + 1}
        aria-valuetext={`Still ${frame + 1} of ${TURNTABLE_FRAMES}`}
        onPointerDown={(e) => {
          dragging.current = true
          e.currentTarget.setPointerCapture?.(e.pointerId)
          onFrame(at(e.clientX))
        }}
        onPointerMove={(e) => {
          if (dragging.current) {
            const k = at(e.clientX)
            if (k !== frame) onFrame(k)
          }
        }}
        onPointerUp={() => {
          dragging.current = false
        }}
        onKeyDown={(e) => {
          const k = e.key === 'ArrowLeft' ? (frame + TURNTABLE_FRAMES - 1) % TURNTABLE_FRAMES : e.key === 'ArrowRight' ? (frame + 1) % TURNTABLE_FRAMES : null
          if (k === null) return
          e.preventDefault()
          e.stopPropagation()
          onFrame(k)
        }}
      >
        {Array.from({ length: TURNTABLE_FRAMES }, (_, k) => (
          <Thumb key={k} source={source} rev={rev} k={k} on={k === frame} />
        ))}
      </div>
      <span className="lv-ttnote">Still {frame + 1} of {TURNTABLE_FRAMES} · drag to turn</span>
    </div>
  )
}
