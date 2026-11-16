// The refusal on the picture (live-scene-v2 mock, "Refused on the render";
// notes v2.2): a callout by the object that was refused, a pin on its box
// and a line to it, in the holder's presence colour, or the warning colour
// when no one holds it (animation drives it). The words and the "Ask …" are
// the shell's (lib/live-view/shared/notices.ts); the code is only in the
// tooltip. It shows on the step the refusal came in, while live.

import { geomOf, worldBox, type BoundsOf, type StillView } from './overlay-geom'
import { presenceSlots, type Slot } from './overlay'
import type { Rect } from './labels'
import type { SimState } from './sim'
import { actorOf } from './wording'

export interface CalloutSpec {
  /** The object refused. */
  a: string
  /** Its box on the still, in the still's pixels. */
  anchor: Rect
  /** The holder's presence slot; null: no holder (the warning colour). */
  slot: Slot | null
  held: boolean
  /** The tooltip: the code, nowhere else. */
  tip: string
}

export function calloutOf(S: SimState, o: { still: StillView; bounds: BoundsOf }): CalloutSpec | null {
  const r = S.callout && S.callout.i === S.i && S.tier === 'live' && S.conn === 'ok' ? S.callout.r : null
  if (!r || !S.D) return null
  const box = worldBox(S.D, r.unit, o.bounds)
  const g = box ? geomOf(box, o.still) : null
  if (!g) return null
  const slot = r.holder ? (presenceSlots(S)[actorOf(r.holder).id] ?? null) : null
  return { a: r.unit, anchor: g.rect, slot, held: !!r.holder, tip: `Code ${r.code}` }
}
