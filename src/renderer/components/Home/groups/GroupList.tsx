// Home groups: the group list in the Groups dialog (`GroupEditModal`), the
// left pane (220 px), or the whole dialog when it is one pane. Each group
// with its bare 12 px glyph (the accent for the current group), its name
// and its agent count. One tab stop: arrows move, Enter / Space / a click
// selects (and, one pane, opens the group's page), ⌥↑ / ⌥↓ move the focused
// group, and so does a drag (the row lifts to 40% and a line marks where it
// lands). "New group" at the bottom makes the next "Group N".
//
// Presentational: changes go out through callbacks (`useHomeGroups`).

import { useEffect, useRef, useState } from 'react'
import { ChevronRightIcon, PlusIcon } from 'lucide-react'
import { ScrollEdgeFrame } from '@/components/ui/ScrollEdgeFade'
import { K2_ICON_PX, K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { GroupGlyph } from './GroupGlyph'
import type { GroupBarItem } from './HomeGroupBar'

export interface GroupListProps {
  groups: readonly GroupBarItem[]
  /** The group the nav shows (accent). */
  currentId: string
  /** The group whose settings show beside the list. */
  selectedId: string
  /** One pane: a pick opens the group's page (a chevron says so), and the
   *  selection isn't lit. */
  drill?: boolean
  onSelectGroup(id: string): void
  onReorderGroups(ids: string[]): void
  /** Make a group; returns its id (the dialog selects it). */
  onCreate(): string | null
}

/** `ids` with the one at `from` moved to `to`. */
export function moveId(ids: readonly string[], from: number, to: number): string[] {
  const out = [...ids]
  const [moved] = out.splice(from, 1)
  out.splice(Math.max(0, Math.min(out.length, to)), 0, moved)
  return out
}

const ROW =
  'relative flex h-[34px] w-full cursor-pointer items-center gap-2.5 px-2.5 text-left text-[12px] text-[var(--color-text-secondary)] hover:bg-[var(--color-row-hover)] hover:text-[var(--color-text-primary)] focus:outline-none [[data-style=glass]_&]:rounded-[calc(var(--radius-box)/2)]'
const ROW_SELECTED = 'bg-[var(--color-wash-2)] font-semibold text-[var(--color-text-primary)]'

function DropLine(): React.JSX.Element {
  return <li aria-hidden="true" role="presentation" data-group-drop-line="" className="relative z-[4] mx-1.5 -my-px h-0.5 list-none bg-[var(--color-text-primary)]" />
}

export default function GroupList({
  groups,
  currentId,
  selectedId,
  drill = false,
  onSelectGroup,
  onReorderGroups,
  onCreate,
}: GroupListProps): React.JSX.Element {
  const ids = groups.map((g) => g.id)
  const [dragId, setDragId] = useState<string | null>(null)
  const [overAt, setOverAt] = useState<number | null>(null)
  const [activeId, setActiveId] = useState(selectedId)
  const listRef = useRef<HTMLUListElement>(null)
  useEffect(() => setActiveId(selectedId), [selectedId])
  const active = ids.includes(activeId) ? activeId : selectedId
  // Focus follows the roving stop while it is in the list.
  useEffect(() => {
    const ul = listRef.current
    if (!ul || !ul.contains(document.activeElement)) return
    Array.from(ul.querySelectorAll<HTMLElement>('[data-group-list-row]'))
      .find((el) => el.dataset.groupListRow === active)
      ?.focus({ preventScroll: false })
  }, [active])

  const dragFrom = dragId ? ids.indexOf(dragId) : -1
  // The line sits where the row would land: above the hovered row when
  // moving up, below it when moving down.
  const lineAt = dragId !== null && overAt !== null && overAt !== dragFrom ? (overAt > dragFrom ? overAt + 1 : overAt) : null

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-group-list="">
      <ScrollEdgeFrame className="min-h-0 flex-1" scrollClassName="overflow-y-auto overflow-x-hidden p-2" tint="var(--color-bg-surface)" testId="group-list-frame">
        <ul ref={listRef} className="m-0 flex list-none flex-col p-0" role="listbox" aria-label="Groups" data-group-list-rows="">
          {groups.map((g, i) => {
            const selected = g.id === selectedId
            const lit = selected && !drill
            return [
              lineAt === i ? <DropLine key={`line-${g.id}`} /> : null,
              <li
                key={g.id}
                role="option"
                tabIndex={g.id === active ? 0 : -1}
                draggable
                aria-selected={selected}
                aria-label={`${g.name}${g.id === currentId ? ', current group' : ''}, ${i + 1} of ${groups.length}${g.recentFirst ? ', most recent at the top' : ''}. Option and arrow keys move it.`}
                data-group-list-row={g.id}
                data-selected={selected ? '' : undefined}
                data-current={g.id === currentId ? '' : undefined}
                data-dragging={dragId === g.id ? '' : undefined}
                data-k2-focus=""
                className={`${ROW} ${lit ? ROW_SELECTED : ''} data-[dragging]:opacity-40`}
                onClick={() => {
                  setActiveId(g.id)
                  onSelectGroup(g.id)
                }}
                onDragStart={(e) => {
                  setDragId(g.id)
                  e.dataTransfer?.setData('text/plain', g.id)
                }}
                onDragEnd={() => {
                  setDragId(null)
                  setOverAt(null)
                }}
                onDragOver={(e) => {
                  if (!dragId) return
                  e.preventDefault()
                  if (overAt !== i) setOverAt(i)
                }}
                onDrop={(e) => {
                  e.preventDefault()
                  setOverAt(null)
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
                  else {
                    setActiveId(ids[to])
                    if (!drill) onSelectGroup(ids[to])
                  }
                }}
              >
                <span
                  className={`flex w-4 flex-shrink-0 justify-center ${
                    g.id === currentId
                      ? 'text-[var(--color-accent-text)]'
                      : lit
                        ? 'text-[var(--color-text-secondary)]'
                        : 'text-[var(--color-text-muted)]'
                  }`}
                >
                  <GroupGlyph icon={g.icon} index={i} size={12} bare />
                </span>
                <span className="min-w-0 flex-1 truncate" data-group-list-name="">
                  {g.name}
                </span>
                {g.recentFirst && (
                  <span className="sr-only" data-group-list-recent="">
                    Most recent at the top
                  </span>
                )}
                {g.count !== undefined && (
                  <span
                    aria-hidden="true"
                    data-group-list-count=""
                    className={`flex-shrink-0 text-[10px] font-normal tabular-nums ${lit ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-text-muted)]'}`}
                  >
                    {g.count}
                  </span>
                )}
                {drill && (
                  <ChevronRightIcon size={14} strokeWidth={K2_ICON_STROKE} strokeLinecap="square" strokeLinejoin="miter" aria-hidden="true" className="flex-shrink-0 text-[var(--color-text-muted)]" />
                )}
              </li>,
            ]
          })}
          {lineAt === groups.length && <DropLine />}
        </ul>
      </ScrollEdgeFrame>
      <div className="flex-shrink-0 border-t border-[var(--color-border)] px-2 pb-2.5 pt-1.5">
        <button
          type="button"
          data-group-create=""
          data-k2-focus=""
          className="no-drag flex h-7 cursor-pointer items-center gap-1.5 px-2 text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-row-hover)] hover:text-[var(--color-text-primary)] focus:outline-none [[data-style=glass]_&]:rounded-[var(--radius-pill)]"
          onClick={() => {
            const id = onCreate()
            if (id) onSelectGroup(id)
          }}
        >
          <PlusIcon size={K2_ICON_PX} strokeWidth={K2_ICON_STROKE} strokeLinecap="square" strokeLinejoin="miter" aria-hidden="true" />
          New group
        </button>
      </div>
    </div>
  )
}
