// Home groups: the group list in the Groups dialog (`GroupEditModal`), the
// left pane (220 px), or the whole dialog when it is one pane. Each group
// with its bare 12 px glyph (the accent for the current group), its name
// and its agent count. One tab stop: arrows move, Enter / Space / a click
// selects (and, one pane, opens the group's page), ⌥↑ / ⌥↓ move the focused
// group, and so does a drag (the row stays at 40% and the list opens a gap
// the row's height where it will land, so no row hides under it). "New
// group" at the bottom makes the next "Group N"; after a delete, "<Name>
// deleted." with Undo sits just above it.
//
// Presentational: changes go out through callbacks (`useHomeGroups`).

import { useEffect, useRef, useState } from 'react'
import { ChevronRightIcon, PlusIcon } from 'lucide-react'
import { DIALOG_EDGE_TINT, ScrollEdgeFrame } from '@/components/ui/ScrollEdgeFade'
import { K2_ICON_PX, K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { GroupGlyph } from './GroupGlyph'
import type { GroupBarItem } from './HomeGroupBar'
import { GROUP_DIALOG_FOOT_CLASS } from './group-bar-look'
import { GROUPS_LABEL, NEW_GROUP_LABEL } from './group-copy'

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
  /** The last delete, offered back until the dialog closes. */
  undo?: GroupListUndo
}

export interface GroupListUndo {
  /** The deleted group's name. */
  name: string
  onUndo(): void
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

/** A row's height, the gap's when the dragged row can't be measured. */
const ROW_PX = 34

/** The gap a dragged group will land in: the row's height, empty. It takes
 *  the drag over it, so the gap holds while the pointer is in it. */
function DropGap({ size, onDragOver, onDrop }: { size: number; onDragOver(e: React.DragEvent): void; onDrop(e: React.DragEvent): void }): React.JSX.Element {
  return <li aria-hidden="true" role="presentation" data-group-drop-gap="" className="list-none" style={{ height: size }} onDragOver={onDragOver} onDrop={onDrop} />
}

export default function GroupList({
  groups,
  currentId,
  selectedId,
  drill = false,
  onSelectGroup,
  onReorderGroups,
  onCreate,
  undo,
}: GroupListProps): React.JSX.Element {
  const ids = groups.map((g) => g.id)
  const [dragId, setDragId] = useState<string | null>(null)
  const [dragSize, setDragSize] = useState(ROW_PX)
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
  // The gap opens where the row would land: above the hovered row when
  // moving up, below it when moving down.
  const gapAt = dragId !== null && overAt !== null && overAt !== dragFrom ? (overAt > dragFrom ? overAt + 1 : overAt) : null
  const dropAt = (at: number): void => {
    setOverAt(null)
    if (!dragId || at === ids.indexOf(dragId)) return
    onReorderGroups(moveId(ids, ids.indexOf(dragId), at))
    setDragId(null)
  }
  const gap = (key: string): React.JSX.Element => (
    <DropGap
      key={key}
      size={dragSize}
      onDragOver={(e) => {
        if (dragId) e.preventDefault()
      }}
      onDrop={(e) => {
        e.preventDefault()
        if (overAt !== null) dropAt(overAt)
      }}
    />
  )

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-group-list="">
      <ScrollEdgeFrame className="min-h-0 flex-1" scrollClassName="overflow-y-auto overflow-x-hidden p-2" tint={DIALOG_EDGE_TINT} heavy testId="group-list-frame">
        <ul ref={listRef} className="m-0 flex list-none flex-col p-0" role="listbox" aria-label={GROUPS_LABEL} data-group-list-rows="">
          {groups.map((g, i) => {
            const selected = g.id === selectedId
            const lit = selected && !drill
            return [
              gapAt === i ? gap(`gap-${g.id}`) : null,
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
                  setDragSize(e.currentTarget.offsetHeight || ROW_PX)
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
                  dropAt(i)
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
          {gapAt === groups.length && gap('gap-end')}
        </ul>
      </ScrollEdgeFrame>
      {undo && (
        <div role="status" className="flex flex-shrink-0 items-center gap-2 border-t border-[var(--color-border)] py-1.5 pl-4 pr-2 type-hint text-[var(--color-text-secondary)]" data-group-undo="">
          <span className="min-w-0 flex-1 truncate">{undo.name} deleted.</span>
          <button
            type="button"
            data-group-undo-button=""
            data-k2-focus=""
            className="no-drag flex h-7 flex-shrink-0 cursor-pointer items-center px-2 text-[var(--color-text-secondary)] hover:bg-[var(--color-row-hover)] hover:text-[var(--color-text-primary)] focus:outline-none [[data-style=glass]_&]:rounded-[var(--radius-pill)]"
            onClick={undo.onUndo}
          >
            Undo
          </button>
        </div>
      )}
      <div className={`${GROUP_DIALOG_FOOT_CLASS} px-2`} data-group-dialog-foot="list">
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
          {NEW_GROUP_LABEL}
        </button>
      </div>
    </div>
  )
}
