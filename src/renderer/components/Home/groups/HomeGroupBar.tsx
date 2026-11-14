// Home groups: the row of group icons at the bottom of the Home nav (under
// the agent list, which fades out behind it; upright in the collapsed 48 px
// rail). Bare icons, no cell behind them, centred, at GROUP_BAR_ICON_PX:
// the current group in the accent colour, the others muted, a numbered dot
// for a group with no icon. Each icon's click target is larger than its
// glyph and keeps the app's focus ring. A click switches. A right-click, a
// long press on touch, or the context-menu key (Shift F10) on an icon opens
// the All groups modal (`GroupEditModal`) with that group selected; the
// nav header's "…" opens it too (`editing`). With many groups the row
// scrolls and keeps the current one in view; ⌥⌘1–9 still switch groups
// (`useWorkspaceIndexShortcuts`), and each icon's tooltip says its chord.
//
// Presentational, for K2 Design to restyle: groups and callbacks in, only
// which modal is open held here when uncontrolled. The logic is
// `useHomeGroups`. The swipe (`useGroupSwipe`) reaches it as `swipe`
// ({progress 0–1, direction next|prev}, null at rest) and `swipeRelease`
// (commit or cancel, once per swipe): the root carries
// `--group-swipe-progress`, `data-swipe-direction` (next, prev or none) and
// `data-swipe-armed` past SWIPE_COMMIT_AT (letting go would switch).

import { useEffect, useRef, useState } from 'react'
import { SWIPE_COMMIT_AT, type SwipeProgress, type SwipeRelease } from '@/lib/group-swipe'
import { GROUP_BAR_ICON_PX, GroupGlyph } from './GroupGlyph'
import GroupEditModal from './GroupEditModal'

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

/** The All groups modal is open, with this group selected. */
export interface GroupEditing {
  id: string
}

export interface HomeGroupBarProps {
  groups: readonly GroupBarItem[]
  currentId: string
  onSelect(id: string): void
  onRename(id: string, name: string): void
  onSetIcon(id: string, icon: string | null): void
  /** Make a group; returns its id (the modal selects it), or null. */
  onCreate(): string | null
  onDelete(id: string): void
  /** "Most recent at the top" on or off. */
  onSetRecentFirst(id: string, on: boolean): void
  /** The groups in a new order. */
  onReorderGroups(ids: string[]): void
  /** Pins live on the agent rows (the Home nav's row menu and drag); the
   *  bar only shows `pinned`. Here for one contract with the row list. */
  onPin?(groupId: string, rowId: string): void
  onUnpin?(groupId: string, rowId: string): void
  onReorderPins?(groupId: string, rowIds: string[]): void
  swipe: SwipeProgress
  /** How the last swipe ended (`seq` grows per release), for the slide. */
  swipeRelease?: (SwipeRelease & { seq: number }) | null
  /** `vertical` in the collapsed rail. */
  orientation?: 'horizontal' | 'vertical'
  /** Controlled modal state (the nav header's "…" opens it too). */
  editing?: GroupEditing | null
  onEditingChange?(next: GroupEditing | null): void
  /** What an empty name saves as (the name field's placeholder). */
  emptyNameFallback?(id: string): string
  /** The switch chord painted in a group's tooltip (`⌥⌘1`), or null. */
  shortcutLabel?(index: number): string | null
}

const LONG_PRESS_MS = 500

const ITEM =
  'no-drag relative flex h-6 w-6 flex-shrink-0 items-center justify-center transition-colors cursor-pointer focus:outline-none'
const ITEM_REST = 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
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
  emptyNameFallback,
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
  const swipeSign = swipe ? (swipe.direction === 'next' ? 1 : -1) : 0

  const buttonFor = (id: string): HTMLElement | null =>
    Array.from(rootRef.current?.querySelectorAll<HTMLElement>('[data-group-id]') ?? []).find(
      (el) => el.dataset.groupId === id,
    ) ?? null

  // Keep the current group in view when the row scrolls.
  useEffect(() => {
    buttonFor(currentId)?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' })
  }, [currentId, groups.length])

  const cancelPress = (): void => {
    if (pressTimer.current) clearTimeout(pressTimer.current)
    pressTimer.current = null
  }
  useEffect(() => cancelPress, [])

  const open = (id: string): void => setEditing({ id })

  const focusSibling = (from: HTMLElement, delta: number): void => {
    const items = Array.from(rootRef.current?.querySelectorAll<HTMLElement>('[data-group-id]') ?? [])
    const i = items.indexOf(from)
    items[Math.min(items.length - 1, Math.max(0, i + delta))]?.focus()
  }

  return (
    <div
      ref={rootRef}
      role="toolbar"
      aria-label="Groups"
      aria-orientation={orientation}
      data-home-group-bar={orientation}
      data-swipe-direction={swipe?.direction ?? 'none'}
      data-swipe-armed={swipe && swipe.progress >= SWIPE_COMMIT_AT ? '' : undefined}
      style={{ '--group-swipe-progress': swipe?.progress ?? 0 } as React.CSSProperties}
      className={`flex min-w-0 items-center justify-center ${vertical ? 'flex-col' : 'flex-row'}`}
    >
      <div
        className={`flex min-h-0 min-w-0 items-center [scrollbar-width:none] ${
          vertical ? 'max-h-48 flex-col overflow-y-auto overflow-x-hidden' : 'flex-row justify-center overflow-x-auto overflow-y-hidden'
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
              data-recent-first={g.recentFirst ? '' : undefined}
              data-k2-focus=""
              aria-label={g.name}
              aria-description={g.recentFirst ? 'Most recent at the top' : undefined}
              aria-current={current ? 'true' : undefined}
              title={`${chord ? `${g.name} (${chord})` : g.name}${g.recentFirst ? ' · Most recent at the top' : ''}`}
              className={`${ITEM} ${current ? ITEM_CURRENT : ITEM_REST}`}
              onClick={() => onSelect(g.id)}
              onContextMenu={(e) => {
                e.preventDefault()
                open(g.id)
              }}
              onPointerDown={(e) => {
                if (e.pointerType !== 'touch') return
                cancelPress()
                pressTimer.current = setTimeout(() => open(g.id), LONG_PRESS_MS)
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
                  open(g.id)
                }
              }}
            >
              <span
                className="flex items-center justify-center"
                data-group-glyph=""
                style={
                  current && swipeSign !== 0
                    ? {
                        transform: vertical
                          ? `translateY(calc(var(--group-swipe-progress) * ${swipeSign} * 50%))`
                          : `translateX(calc(var(--group-swipe-progress) * ${swipeSign} * 50%))`,
                      }
                    : undefined
                }
              >
                <GroupGlyph icon={g.icon} index={i} size={GROUP_BAR_ICON_PX} />
              </span>
              {g.recentFirst && (
                // The "Recent first" mark: a small dot beside the icon.
                <span
                  aria-hidden="true"
                  data-recent-first-mark=""
                  className={`pointer-events-none absolute h-[3px] w-[3px] rounded-full bg-current opacity-70 ${vertical ? 'right-0.5' : 'bottom-0.5'}`}
                />
              )}
            </button>
          )
        })}
      </div>

      {editing && (
        <GroupEditModal
          groups={groups}
          currentId={currentId}
          initialId={editing.id}
          onRename={onRename}
          onSetIcon={onSetIcon}
          onCreate={onCreate}
          onDelete={onDelete}
          onSetRecentFirst={onSetRecentFirst}
          onReorderGroups={onReorderGroups}
          emptyNameFallback={emptyNameFallback}
          onClose={() => setEditing(null)}
        />
      )}
    </div>
  )
}
