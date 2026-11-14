// Home groups: the group list on one side of the All groups modal
// (`GroupEditModal`). Every group with its icon (or numbered dot), its name
// and a mark when "Most recent at the top" is on. Click (or Enter / Space)
// selects a group, and its settings show beside the list. Drag a row, or
// ⌥↑ / ⌥↓ on a focused row, to move it (the bar, the swipe and ⌥⌘1–9
// follow the new order). "Add group" at the end makes the next "Group N".
//
// Presentational: changes go out through callbacks (`useHomeGroups`), so
// K2 Design can restyle it.

import { useState } from 'react'
import { GroupGlyph } from './GroupGlyph'
import type { GroupBarItem } from './HomeGroupBar'

export interface GroupListProps {
  groups: readonly GroupBarItem[]
  /** The group the nav shows (accent). */
  currentId: string
  /** The group whose settings show beside the list. */
  selectedId: string
  onSelectGroup(id: string): void
  onReorderGroups(ids: string[]): void
  /** Make a group; returns its id (the modal selects it). */
  onCreate(): string | null
}

/** `ids` with the one at `from` moved to `to`. */
export function moveId(ids: readonly string[], from: number, to: number): string[] {
  const out = [...ids]
  const [moved] = out.splice(from, 1)
  out.splice(Math.max(0, Math.min(out.length, to)), 0, moved)
  return out
}

export default function GroupList({
  groups,
  currentId,
  selectedId,
  onSelectGroup,
  onReorderGroups,
  onCreate,
}: GroupListProps): React.JSX.Element {
  const ids = groups.map((g) => g.id)
  const [dragId, setDragId] = useState<string | null>(null)

  return (
    <div className="flex min-h-0 flex-col gap-1" data-group-list="">
      <ul className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto" role="listbox" aria-label="Groups" data-group-list-rows="">
        {groups.map((g, i) => {
          const selected = g.id === selectedId
          return (
            <li
              key={g.id}
              role="option"
              tabIndex={selected ? 0 : -1}
              draggable
              aria-selected={selected}
              aria-label={`${g.name}, ${i + 1} of ${groups.length}${g.recentFirst ? ', most recent at the top' : ''}. Option and arrow keys move it.`}
              data-group-list-row={g.id}
              data-selected={selected ? '' : undefined}
              data-dragging={dragId === g.id ? '' : undefined}
              data-k2-focus=""
              className="flex cursor-pointer items-center gap-2 px-2 py-1.5 text-xs text-[var(--color-text-secondary)] hover:bg-[var(--color-wash-1)] focus:outline-none data-[dragging]:opacity-50 data-[selected]:bg-[var(--color-wash-1)] data-[selected]:text-[var(--color-text-primary)]"
              onClick={() => onSelectGroup(g.id)}
              onDragStart={(e) => {
                setDragId(g.id)
                e.dataTransfer?.setData('text/plain', g.id)
              }}
              onDragEnd={() => setDragId(null)}
              onDragOver={(e) => {
                if (dragId) e.preventDefault()
              }}
              onDrop={(e) => {
                e.preventDefault()
                if (!dragId || dragId === g.id) return
                onReorderGroups(moveId(ids, ids.indexOf(dragId), i))
                setDragId(null)
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault()
                  onSelectGroup(g.id)
                  return
                }
                if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return
                e.preventDefault()
                const to = e.key === 'ArrowUp' ? i - 1 : i + 1
                if (to < 0 || to >= ids.length) return
                if (e.altKey) onReorderGroups(moveId(ids, i, to))
                else onSelectGroup(ids[to])
              }}
            >
              <span aria-hidden="true" className="cursor-grab text-[var(--color-text-muted)]" data-group-drag-handle="">
                ⠿
              </span>
              <span className={`flex h-5 w-5 flex-shrink-0 items-center justify-center ${g.id === currentId ? 'text-[var(--color-accent)]' : ''}`}>
                <GroupGlyph icon={g.icon} index={i} />
              </span>
              <span className="min-w-0 flex-1 truncate" data-group-list-name="">
                {g.name}
              </span>
              {g.recentFirst && (
                <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]" data-group-list-recent="" title="Most recent at the top">
                  ⇅ Recent
                </span>
              )}
            </li>
          )
        })}
      </ul>
      <button
        type="button"
        data-group-create=""
        data-k2-focus=""
        className="flex items-center gap-1.5 px-2 py-1.5 text-left text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-primary)] focus:outline-none cursor-pointer"
        onClick={() => {
          const id = onCreate()
          if (id) onSelectGroup(id)
        }}
      >
        <svg className="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.75} strokeLinecap="square" aria-hidden="true">
          <path d="M12 5v14M5 12h14" />
        </svg>
        Add group
      </button>
    </div>
  )
}
