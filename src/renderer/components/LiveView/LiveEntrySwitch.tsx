// "File | Watch live (preview)": the switch between a file tab's own view
// and its live view. Above the file it is a row of its own (the file view
// has its own toolbar); while watching, the live view's header is the tab's
// only header, so the switch moves into it: a small segmented control at the
// header's right end, before the panel toggle. LivePreviewEntry provides it;
// a header with no provider (a live view mounted on its own) draws nothing.

import { createContext, useContext } from 'react'
import { Segmented } from '@/components/ui'

export const WATCH_LIVE_LABEL = 'Watch live (preview)'

export type LiveEntryMode = 'file' | 'live'

export interface LiveEntrySwitchValue {
  /** What the switch is for, for screen readers ("Scene view"). */
  group: string
  /** The test id prefix: `live-<type>`. */
  testIdPrefix: string
  /** What Watch live plays, in words (its tooltip). */
  hint: string
  mode: LiveEntryMode
  onMode: (m: LiveEntryMode) => void
}

const Ctx = createContext<LiveEntrySwitchValue | null>(null)
export const LiveEntrySwitchProvider = Ctx.Provider

/** `compact`: in a live header, "File | Live" (the header already says it's live; the words fit an 800 px window). */
export function LiveEntrySwitchView({ v, compact = false }: { v: LiveEntrySwitchValue; compact?: boolean }): React.JSX.Element {
  return (
    <Segmented<LiveEntryMode>
      aria-label={v.group}
      value={v.mode}
      onChange={v.onMode}
      options={[
        { value: 'file', label: 'File', buttonProps: { title: 'Back to the file' } },
        {
          value: 'live',
          label: compact ? 'Live' : WATCH_LIVE_LABEL,
          buttonProps: { 'data-testid': `${v.testIdPrefix}-watch`, title: `${WATCH_LIVE_LABEL}. ${v.hint}`, ...(compact ? { 'aria-label': 'Live (preview)' } : {}) },
        },
      ]}
    />
  )
}

/** The switch inside a live header, when the header sits in a file tab. */
export function LiveEntrySwitch(): React.JSX.Element | null {
  const v = useContext(Ctx)
  return v ? (
    <span data-testid={`${v.testIdPrefix}-switch`} data-k2-line="" className="lv-entry-switch inline-flex flex-none">
      <LiveEntrySwitchView v={v} compact />
    </span>
  ) : null
}
