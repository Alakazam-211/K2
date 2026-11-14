// Home groups: the current group's name at the top of the Home nav (where
// the Home picker was), with its agent count and a small "…" that opens the
// group's edit card. Switching groups is the bottom bar, the swipe and
// ⌥⌘1–9. Presentational (`useHomeGroups` wires it).

import { PickerTriggerLabel } from '@/components/Sidebar/picker-label'
import { GroupGlyph } from './GroupGlyph'

export interface GroupNameHeaderProps {
  name: string
  icon: string | null
  /** The group's 0-based position (its dot's number). */
  index: number
  count: number
  onEdit(anchor: HTMLElement): void
}

export default function GroupNameHeader({ name, icon, index, count, onEdit }: GroupNameHeaderProps): React.JSX.Element {
  return (
    <div className="flex w-full items-center gap-2 px-2 py-1.5" data-group-name-header="">
      <span className="flex-shrink-0 text-[var(--color-accent)]">
        <GroupGlyph icon={icon} index={index} />
      </span>
      <div className="flex min-w-0 flex-1 items-center gap-2" data-group-name="">
        <PickerTriggerLabel name={name} count={count} />
      </div>
      <button
        type="button"
        aria-label={`Edit group ${name}`}
        title="Edit group"
        data-group-edit-open=""
        className="no-drag flex h-5 w-5 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-primary)] focus:outline-none focus-visible:ring-1 focus-visible:ring-[var(--color-accent)] cursor-pointer"
        onClick={(e) => onEdit(e.currentTarget)}
      >
        <svg className="h-3.5 w-3.5" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <rect x="4" y="11" width="3" height="3" />
          <rect x="10.5" y="11" width="3" height="3" />
          <rect x="17" y="11" width="3" height="3" />
        </svg>
      </button>
    </div>
  )
}
