// Home groups: the All groups modal, centred on the window. It opens from a
// right-click (or a long press, or the context-menu key) on a group's icon
// in the bar, with that group selected, and from the "…" beside the
// current group's name. The group list sits on one side (`GroupList`:
// select, reorder by drag or ⌥↑/⌥↓, Add group); the selected group's
// settings sit beside it (`GroupDetailPane`: name, "Most recent at the
// top", the whole icon set, Delete). Esc, the × or a press outside close it.
//
// Presentational: it holds only which group is selected; every change goes
// out through the callbacks (`useHomeGroups`), so K2 Design can restyle it.
// Portalled to <body>, so no frosted or transformed nav ancestor contains it.

import { useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import { DialogFrame, DialogScrim } from '@/components/ui/Dialog'
import { DialogCloseButton } from '@/components/ui/dialog-cancel'
import GroupList from './GroupList'
import GroupDetailPane from './GroupDetailPane'
import type { GroupBarItem } from './HomeGroupBar'

export interface GroupEditModalProps {
  groups: readonly GroupBarItem[]
  currentId: string
  /** The group selected when it opens (the one right-clicked). */
  initialId: string
  onRename(id: string, name: string): void
  onSetIcon(id: string, icon: string | null): void
  onCreate(): string | null
  onDelete(id: string): void
  onSetRecentFirst(id: string, on: boolean): void
  onReorderGroups(ids: string[]): void
  emptyNameFallback?(id: string): string
  onClose(): void
}

export default function GroupEditModal({
  groups,
  currentId,
  initialId,
  onRename,
  onSetIcon,
  onCreate,
  onDelete,
  onSetRecentFirst,
  onReorderGroups,
  emptyNameFallback,
  onClose,
}: GroupEditModalProps): React.ReactPortal {
  const [selectedId, setSelectedId] = useState(initialId)
  const [justAdded, setJustAdded] = useState<string | null>(null)
  // A deleted (or merged-away) selection falls back to the first group.
  useEffect(() => {
    if (!groups.some((g) => g.id === selectedId) && groups.length > 0) setSelectedId(groups[0].id)
  }, [groups, selectedId])

  const index = Math.max(0, groups.findIndex((g) => g.id === selectedId))
  const selected = groups[index]

  return createPortal(
    <>
      <DialogScrim data-group-edit-scrim="" />
      <DialogFrame
        role="dialog"
        aria-modal="true"
        aria-label="All groups"
        data-group-edit-modal=""
        onCancel={onClose}
        closeButton="inline"
        style={{
          width: 720,
          height: 560,
          maxWidth: 'calc(var(--k2-vw) - 48px)',
          maxHeight: 'calc(var(--k2-vh) - 64px)',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        <div className="flex items-center justify-between border-b border-[var(--color-border)] px-4 py-3">
          <div className="text-sm font-semibold text-[var(--color-text-primary)]">All groups</div>
          <DialogCloseButton className="-mr-2" onClick={onClose} />
        </div>
        <div className="flex min-h-0 flex-1">
          <div className="flex w-56 flex-shrink-0 flex-col border-r border-[var(--color-border)] p-2">
            <GroupList
              groups={groups}
              currentId={currentId}
              selectedId={selected?.id ?? ''}
              onSelectGroup={setSelectedId}
              onReorderGroups={onReorderGroups}
              onCreate={() => {
                const id = onCreate()
                if (id) setJustAdded(id)
                return id
              }}
            />
          </div>
          <div className="flex min-h-0 min-w-0 flex-1 flex-col p-4">
            {selected && (
              <GroupDetailPane
                key={selected.id}
                group={selected}
                index={index}
                canDelete={groups.length > 1}
                onRename={onRename}
                onSetIcon={onSetIcon}
                onSetRecentFirst={onSetRecentFirst}
                onDelete={onDelete}
                emptyNameFallback={emptyNameFallback}
                focusName={justAdded === selected.id}
              />
            )}
          </div>
        </div>
      </DialogFrame>
    </>,
    document.body,
  )
}
