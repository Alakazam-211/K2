// Home groups: the current group's name at the top of the Home nav (where
// the Home picker was), with its agent count and a quiet "…" that opens the
// Groups dialog on it. Plain text, not a menu: switching is the bar, the
// swipe and ⌥⌘1–9. Presentational (`useHomeGroups` wires it).

import { EllipsisIcon } from 'lucide-react'
import { K2_ICON_PX, K2_ICON_STROKE } from '@/components/icons/K2Icons'

export interface GroupNameHeaderProps {
  name: string
  icon: string | null
  /** The group's 0-based position. */
  index: number
  count: number
  /** Open the Groups dialog on this group. */
  onEdit(): void
}

export default function GroupNameHeader({ name, count, onEdit }: GroupNameHeaderProps): React.JSX.Element {
  return (
    <div className="flex min-h-[46px] w-full items-center gap-1.5 pb-2 pl-3 pr-2 pt-3" data-group-name-header="">
      {/* The count sits right after the name, as in every nav picker
          (picker-label.tsx; picker-count.test.tsx). */}
      <span className="flex min-w-0 flex-1 items-baseline gap-1.5" data-picker-label="">
        <span className="min-w-0 truncate text-[13px] font-semibold text-[var(--color-text-primary)]" data-group-name="" data-picker-name="" title={name}>
          {name}
        </span>
        <span className="flex-shrink-0 text-[11px] tabular-nums text-[var(--color-text-muted)]" aria-label={`${count} agents`} data-group-count="" data-picker-count="">
          {count}
        </span>
      </span>
      <button
        type="button"
        aria-label={`Edit groups (${name})`}
        aria-haspopup="dialog"
        title="Edit groups"
        data-group-edit-open=""
        data-k2-focus=""
        className="no-drag flex h-[26px] w-[26px] flex-shrink-0 cursor-pointer items-center justify-center text-[var(--color-text-muted)] hover:bg-[var(--color-row-hover)] hover:text-[var(--color-text-primary)] focus:outline-none [[data-style=glass]_&]:rounded-[var(--radius-pill)]"
        onClick={() => onEdit()}
      >
        <EllipsisIcon size={K2_ICON_PX} strokeWidth={K2_ICON_STROKE} strokeLinecap="square" strokeLinejoin="miter" aria-hidden="true" focusable="false" />
      </button>
    </div>
  )
}
