// The off-screen band (editors-notes.md v3 Spacing): people whose caret is
// above or below the view, as small muted chips in a 34px band at that edge,
// centred over the column. The band takes its own space, so a chip never sits
// on text. A chip jumps to its person. lib/editor/live/offscreen.ts decides
// who is where; the editor measures.

import { EdgeArrow } from '@/components/LiveView/shell/glyphs'
import { LiveAvatar } from '@/components/LiveView/shell/LiveAvatar'
import type { LivePerson } from '@/lib/live-view/shared/people'

export function EdgeBand({ side, people, centre, onJump }: { side: 'top' | 'bottom'; people: readonly LivePerson[]; centre: number; onJump: (id: string) => void }): React.JSX.Element | null {
  if (people.length === 0) return null
  const where = side === 'top' ? 'above' : 'below'
  return (
    <div role="group" aria-label={`People ${where}`} data-testid={`ed-band-${side}`} data-side={side} className="ed-band">
      <div className="ed-band-chips" style={{ left: centre }}>
        {people.map((p) => {
          const label = `${p.name} is ${where}${p.place ? `, in ${p.place}` : ''}. Go to ${p.name}`
          return (
            <button key={p.id} type="button" data-k2-focus="" data-k2-chamfer="chip" data-testid="ed-edge" data-who={p.id} aria-label={label} title={label} onClick={() => onJump(p.id)} className="ed-edge no-drag">
              <LiveAvatar person={p} size={16} />
              {p.name}
              <EdgeArrow side={side} />
            </button>
          )
        })}
      </div>
    </div>
  )
}
