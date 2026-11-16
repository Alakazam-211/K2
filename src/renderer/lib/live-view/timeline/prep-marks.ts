// What a clip on the lanes shows while video is being prepared (T6; the
// design section's Preparing video), pure:
//   - a clip that isn't ready yet is a plain block in its media colour with
//     its name (no filmstrip, no head tile);
//   - a clip that just got its pictures fades them in (150 ms);
//   - a clip nobody can preview carries the no-preview glyph, and is
//     highlighted while the monitor's "can't be previewed" line shows them;
//     they never count as waiting.
// Outside preparing only the no-preview glyph applies (a source the host
// can't read).

import type { Item, TimelineModel } from './fold'
import { prepOf, type MediaFacts, type PreviewState } from './picture'

export interface PrepMarks {
  unready: (it: Item) => boolean
  fresh: (it: Item) => boolean
  noPreview: (it: Item) => boolean
  highlight: (it: Item) => boolean
}

export function prepMarks(M: TimelineModel, pv: PreviewState, facts: MediaFacts): PrepMarks {
  const P = prepOf(M, pv, facts)
  const cant = new Set(P ? P.cant.map((x) => x.key) : [])
  const pool = new Set(P ? P.pool.map((x) => x.key) : [])
  const fresh = new Set(P ? P.fresh : [])
  const noSource = new Set(pv.nopreview || [])
  const noPreview = (it: Item): boolean => cant.has(it.key) || (!!it.media && noSource.has(it.media))
  return {
    unready: (it) => !!P && pool.has(it.key) && !P.isReady(it.key),
    fresh: (it) => fresh.has(it.key),
    noPreview,
    highlight: (it) => !!P && !!pv.prep?.hl && !P.done && cant.has(it.key),
  }
}
