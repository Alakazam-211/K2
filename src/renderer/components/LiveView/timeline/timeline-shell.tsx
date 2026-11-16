// The live timeline tab on the shared shell: the header (its sentence takes
// its own row when narrow), the side panel (Watching, Messages, Changes),
// the notices and the say row, always there while live so the lanes never
// jump. The content (TimelineHero) places the say row and the panel itself:
// the monitor and its transport (T4) with the say row under them, beside
// the panel, then the lanes and ruler (T3) full width under all of it
// (build-live-timeline.md).

import { personById } from '@/lib/live-view/shared/people'
import { FilmGlyph } from '../shell/glyphs'
import type { ShellConfig } from '../shell/shell-config'
import { TimelineHero } from './TimelineHero'

export const TIMELINE_SHELL: ShellConfig = {
  kind: 'timeline',
  kindLabel: 'Timeline',
  fileGlyph: <FilmGlyph size={15} />,
  bannerLine: 'Played from K2’s sample edit; nothing here comes from the file you opened.',
  panel: [
    { id: 'watching', title: 'Watching' },
    { id: 'messages', title: 'Messages' },
    { id: 'changes', title: 'Changes' },
  ],
  narrowBelow: 1100,
  sentenceRowWhenNarrow: true,
  backInHeader: false,
  sayAlways: true,
  // The lanes run full width under the say row and the panel (the mock's layout).
  placesSayAndPanel: true,
  kept: 'who’s watching, the changes and your playhead',
  sayAddress: (snap, to) => {
    const p = personById(snap.people, to)
    const place = p?.place ?? null
    if (!place) return { label: 'the edit', title: 'the whole edit', address: '@timeline' }
    return { label: place.replace(/ on [AV]\d+$/, ''), title: place, address: p?.timePosition?.in ?? '@timeline' }
  },
  Hero: TimelineHero,
}
