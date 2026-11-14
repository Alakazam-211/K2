// Home groups: the bar at the bottom of the Home nav (and, upright, in the
// collapsed 48 px rail) with one small icon per group. The current group is
// in the accent colour, the others muted; a click switches; a group with no
// icon shows its numbered dot; "+" makes a group (named "Group N") and opens
// its card. Right-click, a long press on touch, or the context-menu key on a
// focused icon opens the group's edit card (`GroupEditCard`). With many
// groups the bar scrolls and keeps the current one in view. ⌥⌘1–9 still
// switch groups (`useWorkspaceIndexShortcuts`); each icon's tooltip says its
// chord.
//
// Presentational, for K2 Design to restyle: it takes the groups and
// callbacks and holds only its own UI state (which card is open). The logic
// is `useHomeGroups`. The swipe between groups (`useGroupSwipe`) reaches it
// as `swipe`: the root carries `--group-swipe-progress` (0–1) and
// `data-swipe-direction` (-1, 0, 1) for a visual that follows the finger.

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import type { SwipeProgress } from '@/lib/group-swipe'
import { GroupGlyph } from './GroupGlyph'
import GroupEditCard from './GroupEditCard'

export interface GroupBarItem {
  id: string
  name: string
  icon: string | null
  /** Position in the group list (0-based). */
  order: number
  /** "Most recent at the top" is on: the bar marks the group. */
  recentFirst: boolean
  /** Its pinned rows' addresses, in pin order (at most 9). */
  pinned: readonly string[]
}

/** Which group's card is open, and what it sits against (default: that
 *  group's icon in the bar). */
export interface GroupEditing {
  id: string
  anchor?: HTMLElement | null
}

export interface HomeGroupBarProps {
  groups: readonly GroupBarItem[]
  currentId: string
  onSelect(id: string): void
  onRename(id: string, name: string): void
  onSetIcon(id: string, icon: string | null): void
  /** Make a group; returns its id (its card opens), or null. */
  onCreate(): string | null
  onDelete(id: string): void
  /** "Most recent at the top" on or off (the card's Group settings). */
  onSetRecentFirst(id: string, on: boolean): void
  /** The groups in a new order (the card's Group settings). */
  onReorderGroups(ids: string[]): void
  /** Pins live on the agent rows (the Home nav's row menu and drag); the
   *  bar only shows `pinned`. Here for one contract with the row list. */
  onPin?(groupId: string, rowId: string): void
  onUnpin?(groupId: string, rowId: string): void
  onReorderPins?(groupId: string, rowIds: string[]): void
  swipe: SwipeProgress
  /** `vertical` in the collapsed rail. */
  orientation?: 'horizontal' | 'vertical'
  /** Controlled card state (the nav header's "…" opens it too). */
  editing?: GroupEditing | null
  onEditingChange?(next: GroupEditing | null): void
  /** The switch chord painted in a group's tooltip (`⌥⌘1`), or null. */
  shortcutLabel?(index: number): string | null
}

const LONG_PRESS_MS = 500

const ITEM =
  'no-drag relative flex h-7 w-7 flex-shrink-0 items-center justify-center transition-colors cursor-pointer focus:outline-none focus-visible:ring-1 focus-visible:ring-[var(--color-accent)]'
const ITEM_REST = 'text-[var(--color-text-muted)] hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-secondary)]'
const ITEM_CURRENT = 'text-[var(--color-accent)]'

export default function HomeGroupBar({
  groups,
  currentId,
  onSelect,
  onRename,
  onSetIcon,
  onCreate,
  onDelete,
  onSetRecentFirst,
  onReorderGroups,
  swipe,
  orientation = 'horizontal',
  editing: editingProp,
  onEditingChange,
  shortcutLabel,
}: HomeGroupBarProps): React.JSX.Element {
  const [editingOwn, setEditingOwn] = useState<GroupEditing | null>(null)
  const controlled = editingProp !== undefined
  const editing = controlled ? editingProp : editingOwn
  const setEditing = (next: GroupEditing | null): void => {
    if (!controlled) setEditingOwn(next)
    onEditingChange?.(next)
  }
  const rootRef = useRef<HTMLDivElement>(null)
  const pressTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const vertical = orientation === 'vertical'

  const buttonFor = (id: string): HTMLElement | null =>
    Array.from(rootRef.current?.querySelectorAll<HTMLElement>('[data-group-id]') ?? []).find(
      (el) => el.dataset.groupId === id,
    ) ?? null

  // What the open card sits against, read once the bar is on screen (a
  // group "+" just made has its icon only after this render).
  const [anchorEl, setAnchorEl] = useState<HTMLElement | null>(null)
  useLayoutEffect(() => {
    setAnchorEl(editing ? (editing.anchor ?? buttonFor(editing.id)) : null)
  }, [editing, groups])

  // Keep the current group in view when the bar scrolls.
  useEffect(() => {
    buttonFor(currentId)?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' })
  }, [currentId, groups.length])

  // A card for a group that is gone closes.
  useEffect(() => {
    if (editing && !groups.some((g) => g.id === editing.id)) setEditing(null)
  })

  const cancelPress = (): void => {
    if (pressTimer.current) clearTimeout(pressTimer.current)
    pressTimer.current = null
  }
  useEffect(() => cancelPress, [])

  const open = (id: string, anchor: HTMLElement | null): void => setEditing({ id, anchor })

  const focusSibling = (from: HTMLElement, delta: number): void => {
    const items = Array.from(rootRef.current?.querySelectorAll<HTMLElement>('[data-group-id], [data-group-create]') ?? [])
    const i = items.indexOf(from)
    const next = items[Math.min(items.length - 1, Math.max(0, i + delta))]
    next?.focus()
  }

  const editingIndex = editing ? groups.findIndex((g) => g.id === editing.id) : -1
  const editingGroup = editingIndex >= 0 ? groups[editingIndex] : null
  const editingAnchor = editing ? anchorEl : null

  return (
    <div
      ref={rootRef}
      role="toolbar"
      aria-label="Groups"
      aria-orientation={orientation}
      data-home-group-bar={orientation}
      data-swipe-direction={swipe.direction}
      style={{ '--group-swipe-progress': swipe.progress } as React.CSSProperties}
      className={`flex items-center gap-0.5 ${vertical ? 'flex-col' : 'flex-row'}`}
    >
      <div
        className={`flex min-h-0 min-w-0 items-center gap-0.5 [scrollbar-width:none] ${
          vertical ? 'max-h-48 flex-col overflow-y-auto overflow-x-hidden' : 'flex-1 flex-row justify-center overflow-x-auto overflow-y-hidden'
        }`}
        data-group-bar-scroller=""
      >
        {groups.map((g, i) => {
          const current = g.id === currentId
          const chord = shortcutLabel?.(i) ?? null
          return (
            <button
              key={g.id}
              type="button"
              data-group-id={g.id}
              data-current={current ? '' : undefined}
              aria-label={g.name}
              aria-description={g.recentFirst ? 'Most recent at the top' : undefined}
              aria-current={current ? 'true' : undefined}
              data-recent-first={g.recentFirst ? '' : undefined}
              title={`${chord ? `${g.name} (${chord})` : g.name}${g.recentFirst ? ' · Most recent at the top' : ''}`}
              className={`${ITEM} ${current ? ITEM_CURRENT : ITEM_REST}`}
              onClick={() => onSelect(g.id)}
              onContextMenu={(e) => {
                e.preventDefault()
                open(g.id, e.currentTarget)
              }}
              onPointerDown={(e) => {
                if (e.pointerType !== 'touch') return
                const el = e.currentTarget
                cancelPress()
                pressTimer.current = setTimeout(() => open(g.id, el), LONG_PRESS_MS)
              }}
              onPointerUp={cancelPress}
              onPointerLeave={cancelPress}
              onPointerCancel={cancelPress}
              onKeyDown={(e) => {
                const back = vertical ? 'ArrowUp' : 'ArrowLeft'
                const fwd = vertical ? 'ArrowDown' : 'ArrowRight'
                if (e.key === back || e.key === fwd) {
                  e.preventDefault()
                  focusSibling(e.currentTarget, e.key === fwd ? 1 : -1)
                } else if (e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey)) {
                  e.preventDefault()
                  open(g.id, e.currentTarget)
                }
              }}
            >
              <GroupGlyph icon={g.icon} index={i} />
              {g.recentFirst && (
                // The "Recent first" mark: a small clock hand in the corner.
                <svg
                  aria-hidden="true"
                  data-recent-first-mark=""
                  className="pointer-events-none absolute right-0 top-0 h-2.5 w-2.5"
                  viewBox="0 0 10 10"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth={1.25}
                  strokeLinecap="square"
                >
                  <circle cx="5" cy="5" r="4" />
                  <path d="M5 2.75V5h1.75" />
                </svg>
              )}
              {current && (
                <span
                  aria-hidden="true"
                  data-group-current-mark=""
                  className={`pointer-events-none absolute bg-[var(--color-accent)] ${
                    vertical ? 'left-0 top-1.5 bottom-1.5 w-[2px]' : 'bottom-0 left-1.5 right-1.5 h-[2px]'
                  }`}
                  style={{
                    transform: vertical
                      ? `translateY(calc(var(--group-swipe-progress) * ${swipe.direction} * 50%))`
                      : `translateX(calc(var(--group-swipe-progress) * ${swipe.direction} * 50%))`,
                  }}
                />
              )}
            </button>
          )
        })}
      </div>
      <button
        type="button"
        data-group-create=""
        aria-label="New group"
        title="New group"
        className={`${ITEM} ${ITEM_REST}`}
        onClick={() => {
          const id = onCreate()
          if (id) open(id, null)
        }}
        onKeyDown={(e) => {
          const back = vertical ? 'ArrowUp' : 'ArrowLeft'
          if (e.key === back) {
            e.preventDefault()
            focusSibling(e.currentTarget, -1)
          }
        }}
      >
        <svg className="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.75} strokeLinecap="square" aria-hidden="true">
          <path d="M12 5v14M5 12h14" />
        </svg>
      </button>

      {editingGroup && editingAnchor && (
        <GroupEditCard
          key={editingGroup.id}
          group={editingGroup}
          index={editingIndex}
          canDelete={groups.length > 1}
          anchor={editingAnchor}
          prefer={editing?.anchor && !rootRef.current?.contains(editing.anchor) ? 'down' : 'up'}
          onRename={onRename}
          onSetIcon={onSetIcon}
          onDelete={onDelete}
          onClose={() => setEditing(null)}
          groups={groups}
          currentId={currentId}
          onSetRecentFirst={onSetRecentFirst}
          onReorderGroups={onReorderGroups}
          onEditGroup={(id) => setEditing({ id, anchor: editingAnchor })}
        />
      )}
    </div>
  )
}
