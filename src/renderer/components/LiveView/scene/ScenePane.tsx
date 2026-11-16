// One view of the scene on the stage: the letterboxed still, the overlay
// over it, the view picker (and who looks through other views) on its
// top-left corner and the compass on its bottom-left. One fills the stage;
// two split it (follow-view.ts paneRects), each with its own picker and
// compass, so the second can follow the agent's view while you look
// through another.

import type { CSSProperties } from 'react'
import { useRef } from 'react'
import type { BoundsOf } from '@/lib/live-view/scene/overlay'
import type { PaneRect } from '@/lib/live-view/scene/follow-view'
import { coverBox, letterbox, type Still } from '@/lib/live-view/scene/picture-source'
import type { SimState } from '@/lib/live-view/scene/sim'
import type { Doc } from '@/lib/live-view/scene/types'
import type { HeroRefusal } from '../shell/shell-config'
import { Compass } from './Compass'
import { SceneOverlay } from './SceneOverlay'
import { ScenePicture } from './ScenePicture'

export interface ScenePaneProps {
  /** Where the pane sits on the stage; null: the whole stage. */
  rect: PaneRect | null
  /** Second pane: drawn with a divider. */
  index: 0 | 1
  S: SimState
  pick: string | null | undefined
  still: Still | null
  name: string
  dim: boolean
  ground: Doc | null
  /** Boxes over the still (null: none, e.g. no bounds or no still). */
  bounds: BoundsOf | null
  overlay: boolean
  motion: boolean
  onFrame: (a: string) => void
  /** The picker and the chips beside it. */
  top: React.ReactNode
  /** Anything else on this pane (the turntable strip). */
  children?: React.ReactNode
  size: { width: number; height: number }
  /** A menu or the details over this pane: labels wait. */
  quiet?: boolean
  /** The stage's chrome changed: labels are placed again. */
  chromeKey?: string
  /** A refusal to draw by its object (the first pane only). */
  refusal?: HeroRefusal | null
}

export function ScenePane(p: ScenePaneProps): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  const w = p.rect?.w ?? p.size.width
  const h = p.rect?.h ?? p.size.height
  // Half the stage: the still covers the half. The whole stage: letterboxed in its faded margin.
  const lb = p.rect ? coverBox(w, h) : letterbox(w, h)
  const style: CSSProperties | undefined = p.rect ? { left: p.rect.x, top: p.rect.y, width: p.rect.w, height: p.rect.h } : undefined
  return (
    <div ref={ref} className="lv-pane" data-pane={p.index} data-split={p.rect ? '' : undefined} data-view={p.still?.view} style={style}>
      <ScenePicture lb={lb} still={p.still} label={`Clay picture of the scene, ${p.name}`} dim={p.dim} ground={p.ground} motion={p.motion} />
      {p.overlay && p.still?.camera && p.bounds ? <SceneOverlay S={p.S} pick={p.pick} still={p.still} lb={lb} bounds={p.bounds} motion={p.motion} stage={ref} pane={{ width: w, height: h }} onFrame={p.onFrame} quiet={p.quiet} chromeKey={p.chromeKey} refusal={p.refusal} /> : null}
      {p.top}
      {p.still?.camera ? (
        <div className="lv-cmpslot">
          <Compass camera={p.still.camera} />
        </div>
      ) : null}
      {p.children}
    </div>
  )
}
