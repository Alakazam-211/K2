// Home groups: the small captions in the agent list (K2 Design's mock).
// "Pinned" above a group's pinned agents; "Agents" above the rest once
// there are pins or "Most recent at the top" is on, with a quiet
// "· Recent first" mark for the setting. And the pin mark after a pinned
// agent's name (its ⌘ number is the row's own number column).

import { ArrowUpDownIcon } from 'lucide-react'
import { K2_ICON_STROKE, PinIcon } from '@/components/icons/K2Icons'

export function HomeRowCaption({ kind, recentFirst = false }: { kind: 'pinned' | 'agents'; recentFirst?: boolean }): React.JSX.Element {
  return (
    <div
      className="flex items-center gap-1.5 px-3 pb-1 pt-2 text-[10px] font-semibold uppercase tracking-[0.06em] text-[var(--color-text-muted)] [[data-style=glass]_&]:px-3.5"
      data-home-row-caption={kind}
    >
      {kind === 'pinned' ? 'Pinned' : 'Agents'}
      {kind === 'agents' && recentFirst && (
        <span
          className="inline-flex items-center gap-1 font-medium normal-case tracking-normal text-[var(--color-text-secondary)]"
          title="The agent you last messaged comes first"
          data-home-recent-first-mark=""
        >
          ·
          <ArrowUpDownIcon size={12} strokeWidth={K2_ICON_STROKE} strokeLinecap="square" strokeLinejoin="miter" aria-hidden="true" />
          Recent first
        </span>
      )}
    </div>
  )
}

/** The pin after a pinned agent's name. */
export function HomeRowPinMark(): React.JSX.Element {
  return (
    <span className="inline-flex flex-shrink-0 text-[var(--color-text-muted)]" title="Pinned" data-home-row-pin="">
      <PinIcon size={12} />
    </span>
  )
}
