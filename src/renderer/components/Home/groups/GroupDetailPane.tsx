// Home groups: the selected group's settings beside the list in the All
// groups modal. Its name (saved as you type; an empty one saves as the
// next "Group N", shown as the placeholder), "Most recent at the top" (the
// app's Toggle), the whole icon set (search, a Recent row and the full
// curated grid, `GroupIconPicker`), and Delete (confirm once; off on the
// last group).
//
// Presentational: every change goes out through a callback, so K2 Design
// can restyle it.

import { useEffect, useRef, useState } from 'react'
import { Button, Toggle } from '@/components/ui'
import { HOME_NAME_MAX } from '@/stores/homes'
import GroupIconPicker from './GroupIconPicker'
import type { GroupBarItem } from './HomeGroupBar'

export interface GroupDetailPaneProps {
  group: GroupBarItem
  /** The group's 0-based position (its dot's number). */
  index: number
  canDelete: boolean
  onRename(id: string, name: string): void
  onSetIcon(id: string, icon: string | null): void
  onSetRecentFirst(id: string, on: boolean): void
  onDelete(id: string): void
  emptyNameFallback?(id: string): string
  /** Focus the name with its text selected (a group just added). */
  focusName?: boolean
}

export default function GroupDetailPane({
  group,
  index,
  canDelete,
  onRename,
  onSetIcon,
  onSetRecentFirst,
  onDelete,
  emptyNameFallback,
  focusName = false,
}: GroupDetailPaneProps): React.JSX.Element {
  const [name, setName] = useState(group.name)
  const [confirming, setConfirming] = useState(false)
  const nameRef = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (!focusName) return
    nameRef.current?.focus()
    nameRef.current?.select()
  }, [focusName])

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3" data-group-detail={group.id}>
      <label className="flex flex-col gap-1">
        <span className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">Name</span>
        <input
          ref={nameRef}
          value={name}
          maxLength={HOME_NAME_MAX}
          placeholder={emptyNameFallback?.(group.id)}
          data-group-name-input=""
          className="w-full border border-[var(--color-border)] bg-[var(--color-bg-surface)] px-2 py-1 text-xs text-[var(--color-text-primary)] focus:border-[var(--color-accent)] focus:outline-none"
          onChange={(e) => {
            setName(e.target.value)
            onRename(group.id, e.target.value)
          }}
        />
      </label>

      <div className="flex items-center justify-between gap-2 text-[11px] text-[var(--color-text-secondary)]">
        <span id={`group-recent-${group.id}`}>Most recent at the top</span>
        <Toggle
          size="sm"
          checked={group.recentFirst}
          aria-labelledby={`group-recent-${group.id}`}
          data-group-recent-first={group.id}
          onChange={(on) => onSetRecentFirst(group.id, on)}
        />
      </div>

      <div className="flex min-h-0 flex-1 flex-col gap-1">
        <span className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">Icon</span>
        <GroupIconPicker groupId={group.id} icon={group.icon} index={index} onSetIcon={onSetIcon} />
      </div>

      <div className="border-t border-[var(--color-border)] pt-2">
        {confirming ? (
          <div className="flex items-center gap-2" data-group-delete-confirm="">
            <span className="min-w-0 flex-1 truncate text-[11px] text-[var(--color-text-secondary)]">
              Delete “{group.name}”? Its agents stay where they are.
            </span>
            <Button variant="ghost" onClick={() => setConfirming(false)}>
              Cancel
            </Button>
            <Button
              variant="danger"
              data-group-delete-yes=""
              onClick={() => {
                setConfirming(false)
                onDelete(group.id)
              }}
            >
              Delete
            </Button>
          </div>
        ) : (
          <Button
            variant="danger-quiet"
            disabled={!canDelete}
            title={canDelete ? `Delete “${group.name}”` : 'The last group can’t be deleted'}
            data-group-delete=""
            onClick={() => setConfirming(true)}
          >
            Delete group
          </Button>
        )}
      </div>
    </div>
  )
}
