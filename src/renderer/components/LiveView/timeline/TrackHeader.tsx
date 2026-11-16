// A lane's header, read-only (watching only): its short label (V2, A1) in a
// chip coloured by kind, its name, the clip count on large lanes, and a
// muted, turned-off or locked track flagged with a glyph (focusable, with a
// tooltip and an accessible name), never hidden and never a word. Narrow,
// it keeps the chip and the clip count; the name moves to the tooltip.

import type { Track } from '@/lib/live-view/timeline/fold'

/** What a header shows, as plain values (a Track holds BigInt ticks; see box.ts). */
export type TrackFacts = Pick<Track, 'label' | 'title' | 'kind' | 'enabled' | 'muted' | 'locked' | 'clips'>
export const trackFacts = (t: Track): TrackFacts => ({ label: t.label, title: t.title, kind: t.kind, enabled: t.enabled, muted: t.muted, locked: t.locked, clips: t.clips })
import { plural } from '@/lib/live-view/timeline/wording'
import { LockGlyph, MutedGlyph, OffGlyph } from './glyphs'

/** The track's name: its own, else "Video 1" from its label. */
export function trackName(t: TrackFacts): string {
  if (t.title && t.title !== t.label) return t.title
  return `${t.kind === 'video' ? 'Video' : t.kind === 'audio' ? 'Audio' : 'Track'} ${String(t.label).replace(/^\D+/, '')}`.trim()
}

function Flag({ t }: { t: TrackFacts }): React.JSX.Element | null {
  const f = !t.enabled
    ? { label: 'Turned off in the file', tip: 'This track is turned off in the file: the preview passes over it', G: OffGlyph }
    : t.muted
      ? { label: 'Muted in the file', tip: 'This track is muted in the file: it plays silent', G: MutedGlyph }
      : t.locked
        ? { label: 'Locked in the file', tip: 'This track is locked in the file', G: LockGlyph }
        : null
  if (!f) return null
  return (
    <span className="lv-tl-hflag" role="img" tabIndex={0} data-k2-focus="" data-k2-chamfer="chip" aria-label={f.label} title={f.tip} data-testid={`lv-lane-flag-${t.label}`}>
      <f.G size={12} />
    </span>
  )
}

export function TrackHeader({ t, width, narrow, size, height }: { t: TrackFacts; width: number; narrow: boolean; size: 'S' | 'M' | 'L'; height: number }): React.JSX.Element {
  const name = trackName(t)
  const count = plural(t.clips, 'clip')
  if (narrow) {
    return (
      <div className="lv-tl-head" data-narrow="" role="group" aria-label={`${t.label}, ${name}, ${count}`} title={`${t.label} · ${name} · ${count}`} style={{ width }} data-testid={`lv-lane-head-${t.label}`}>
        <span className="inline-flex items-center gap-1">
          <span className="lv-tl-chip" data-k2-chamfer="chip">
            {t.label}
          </span>
          <Flag t={t} />
        </span>
        {height >= 40 ? <span className="lv-caption lv-num">{count}</span> : null}
      </div>
    )
  }
  return (
    <div className="lv-tl-head" role="group" aria-label={`${t.label}, ${name}`} title={`${t.label} · ${name}`} style={{ width }} data-testid={`lv-lane-head-${t.label}`}>
      <span className="lv-tl-chip" data-k2-chamfer="chip">
        {t.label}
      </span>
      <span className="lv-tl-hname">
        <span>{name}</span>
        {size === 'L' ? <span className="lv-caption lv-num">{count}</span> : null}
      </span>
      <Flag t={t} />
    </div>
  )
}
