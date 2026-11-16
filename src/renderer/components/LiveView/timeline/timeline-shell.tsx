// The live timeline tab on the shared shell. Phase 1: the header (its
// sentence takes its own row when narrow), the side panel (Watching,
// Messages, Changes), the notices and the say row, always there while live
// so the lanes never jump. "Back to X" lives in the transport here, so
// Phase 1 keeps a place for it under the monitor. Later phases
// (build-live-timeline.md T2–T8) fill the monitor, transport and lanes.

import { personById } from '@/lib/live-view/shared/people'
import { FilmGlyph } from '../shell/glyphs'
import type { ShellConfig, ShellHeroProps } from '../shell/shell-config'
import { StagePlaceholder } from '../shell/StagePlaceholder'

function TimelineHero(p: ShellHeroProps): React.JSX.Element {
  const line = p.state === 'loading' ? 'Opening the edit. The lanes come first, then the pictures.' : 'The edit’s monitor and lanes show here: everyone’s playhead, and each change as it lands.'
  const below = p.back ? (
    <div data-testid="lv-transport-place" className="flex flex-none items-center justify-center">
      {p.back}
    </div>
  ) : null
  return <StagePlaceholder kind="timeline" line={line} p={p} below={below} />
}

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
  kept: 'who’s watching, the changes and your playhead',
  sayAddress: (snap, to) => {
    const p = personById(snap.people, to)
    const place = p?.place ?? null
    if (!place) return { label: 'the edit', title: 'the whole edit', address: '@timeline' }
    return { label: place.replace(/ on [AV]\d+$/, ''), title: place, address: p?.timePosition?.in ?? '@timeline' }
  },
  Hero: TimelineHero,
}
