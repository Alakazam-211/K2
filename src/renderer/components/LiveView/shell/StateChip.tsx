// The one state treatment: a dot and a word, on K2's tag (kept as written,
// not in capitals). Every live state uses it: Live, Live · picture behind,
// Loading, Reconnecting, Can't reconnect, Offline, Edits paused, File
// missing, Read only. The dot is round in both looks (status dots are
// Terminal's one rounded mark); in-between states pulse unless motion is
// reduced.

import { Tag } from '@/components/ui'
import { stateChip, type LiveTabState } from '@/lib/live-view/shared/state'

const PULSE: ReadonlySet<LiveTabState> = new Set(['loading', 'reconnecting'])

/** A short why rides after the word; a long one only in the tooltip, so the header never overflows. */
const WHY_INLINE_MAX = 32

/** `why`: the state's reason ("Read only: K2 writes back .otio only"); `short` keeps just the word (the title keeps the rest). */
export function StateChip({ state, why, short = false }: { state: LiveTabState; why?: string | null; short?: boolean }): React.JSX.Element {
  const { word, tone } = stateChip(state)
  const full = why ? `${word}: ${why}` : word
  const inline = !short && !!why && why.length <= WHY_INLINE_MAX
  return (
    <Tag tone={tone} data-case="as-written" data-testid="lv-state" data-state={state} className="lv-state inline-flex flex-none items-center gap-1.5 whitespace-nowrap" title={full}>
      <span data-lv-dot="" data-pulse={PULSE.has(state) ? '' : undefined} aria-hidden="true" className="lv-dot" />
      {inline ? full : word}
    </Tag>
  )
}
