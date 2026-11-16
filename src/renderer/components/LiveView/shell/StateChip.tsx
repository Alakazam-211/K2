// The one state treatment: a dot and a word, on K2's tag (kept as written,
// not in capitals). Every live state uses it: Live, Live · picture behind,
// Loading, Reconnecting, Can't reconnect, Offline, Edits paused, File
// missing, Read only. The dot is round in both looks (status dots are
// Terminal's one rounded mark); in-between states pulse unless motion is
// reduced.

import { Tag } from '@/components/ui'
import { stateChip, type LiveTabState } from '@/lib/live-view/shared/state'

const PULSE: ReadonlySet<LiveTabState> = new Set(['loading', 'reconnecting'])

export function StateChip({ state }: { state: LiveTabState }): React.JSX.Element {
  const { word, tone } = stateChip(state)
  return (
    <Tag tone={tone} data-case="as-written" data-testid="lv-state" data-state={state} className="lv-state inline-flex flex-none items-center gap-1.5 whitespace-nowrap" title={word}>
      <span data-lv-dot="" data-pulse={PULSE.has(state) ? '' : undefined} aria-hidden="true" className="lv-dot" />
      {word}
    </Tag>
  )
}
