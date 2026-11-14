// Home groups: the edit card's "Group settings" view. Every group, with its
// icon, its name (saved as you type), its "Most recent at the top" switch,
// and its place in the list: drag a row, or ⌥↑ / ⌥↓ on a focused row, to
// move it (the bar, the swipe and ⌥⌘1–9 follow the new order). Clicking a
// group's icon opens that group's own card view (name, icon, Delete).
//
// Presentational: every change goes out through a callback
// (`useHomeGroups` wires them), so K2 Design can restyle it.

import { useState } from 'react'
import { HOME_NAME_MAX } from '@/stores/homes'
import { GroupGlyph } from './GroupGlyph'
import type { GroupBarItem } from './HomeGroupBar'

export interface GroupSettingsListProps {
  groups: readonly GroupBarItem[]
  currentId: string
  onRename(id: string, name: string): void
  onSetRecentFirst(id: string, on: boolean): void
  onReorderGroups(ids: string[]): void
  /** Open one group's own view (its icon grid and Delete). */
  onEditGroup(id: string): void
  onBack(): void
}

/** `ids` with the one at `from` moved to `to`. */
export function moveId(ids: readonly string[], from: number, to: number): string[] {
  const out = [...ids]
  const [moved] = out.splice(from, 1)
  out.splice(Math.max(0, Math.min(out.length, to)), 0, moved)
  return out
}

function NameField({ group, onRename }: { group: GroupBarItem; onRename(id: string, name: string): void }): React.JSX.Element {
  const [name, setName] = useState(group.name)
  return (
    <input
      value={name}
      maxLength={HOME_NAME_MAX}
      aria-label={`Name of ${group.name}`}
      data-group-settings-name=""
      className="min-w-0 flex-1 border border-transparent bg-transparent px-1 py-0.5 text-xs text-[var(--color-text-primary)] hover:border-[var(--color-border)] focus:border-[var(--color-accent)] focus:outline-none"
      onChange={(e) => {
        setName(e.target.value)
        if (e.target.value.trim()) onRename(group.id, e.target.value)
      }}
      onKeyDown={(e) => e.stopPropagation()}
    />
  )
}

export default function GroupSettingsList({
  groups,
  currentId,
  onRename,
  onSetRecentFirst,
  onReorderGroups,
  onEditGroup,
  onBack,
}: GroupSettingsListProps): React.JSX.Element {
  const ids = groups.map((g) => g.id)
  const [dragId, setDragId] = useState<string | null>(null)

  return (
    <div className="flex flex-col gap-2" data-group-settings="">
      <div className="flex items-center gap-2">
        <button
          type="button"
          aria-label="Back"
          data-group-settings-back=""
          className="flex h-5 w-5 items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer"
          onClick={onBack}
        >
          <svg className="h-3.5 w-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="square" strokeLinejoin="miter" aria-hidden="true">
            <path d="M15 18l-6-6 6-6" />
          </svg>
        </button>
        <span className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">Group settings</span>
      </div>
      <ul className="flex max-h-72 flex-col gap-0.5 overflow-y-auto" aria-label="Groups" data-group-settings-list="">
        {groups.map((g, i) => (
          <li
            key={g.id}
            tabIndex={0}
            draggable
            aria-label={`${g.name}, ${i + 1} of ${groups.length}. Option and arrow keys move it.`}
            data-group-settings-row={g.id}
            data-dragging={dragId === g.id ? '' : undefined}
            className="flex items-center gap-1.5 px-1 py-1 focus:outline-none focus-visible:ring-1 focus-visible:ring-[var(--color-accent)] data-[dragging]:opacity-50"
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
              if (!e.altKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return
              e.preventDefault()
              const to = e.key === 'ArrowUp' ? i - 1 : i + 1
              if (to < 0 || to >= ids.length) return
              onReorderGroups(moveId(ids, i, to))
            }}
          >
            <span aria-hidden="true" className="cursor-grab px-0.5 text-[var(--color-text-muted)]" data-group-drag-handle="">
              ⠿
            </span>
            <button
              type="button"
              aria-label={`Icon and more for ${g.name}`}
              className={`flex h-6 w-6 flex-shrink-0 items-center justify-center hover:bg-[var(--color-wash-1)] cursor-pointer ${
                g.id === currentId ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-secondary)]'
              }`}
              onClick={() => onEditGroup(g.id)}
            >
              <GroupGlyph icon={g.icon} index={i} />
            </button>
            <NameField group={g} onRename={onRename} />
            <label className="flex flex-shrink-0 items-center gap-1 text-[10px] text-[var(--color-text-muted)] cursor-pointer" title="Most recent at the top">
              <input
                type="checkbox"
                role="switch"
                aria-checked={g.recentFirst}
                aria-label={`Most recent at the top in ${g.name}`}
                checked={g.recentFirst}
                data-group-recent-first={g.id}
                className="cursor-pointer accent-[var(--color-accent)]"
                onChange={(e) => onSetRecentFirst(g.id, e.target.checked)}
              />
              Recent
            </label>
          </li>
        ))}
      </ul>
    </div>
  )
}
