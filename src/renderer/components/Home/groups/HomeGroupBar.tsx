// Home groups: the row of group icons at the bottom of the Home nav, its own
// row above "Add Agent | «" (upright in the collapsed 48 px rail). K2
// Design's look (.k2/prds/mockup-home-groups.html, home-groups-notes.md):
//   - bare Lucide glyphs with nothing behind them, at GROUP_BAR_ICON_PX
//     (the one size constant, 12 px), spaced by the footer's own gap
//     between Add Agent and « (GROUP_BAR_GAP_PX, 8 px): each glyph in a
//     20 × 20 target, the targets touching, never overlapping;
//   - the current group in the accent text colour, the others in muted ink,
//     primary ink on hover, and a tip with the name and its ⌥⌘N;
//   - a group with no icon shows its position number, bare, bold
//     (`groupNumeralPx`: 11 px beside 12 px glyphs);
//   - the app's focus ring round the target (cut in Terminal, a circle in
//     Glass, home-groups.css);
//   - with more groups than fit it scrolls (edge fades, the current group
//     centred); the rail shows at most five and scrolls past that. While
//     they don't fit, a chevron at each end (up/down in the rail) shows
//     while more groups sit that way and pages the bar: pointer only, out
//     of the tab order (group-bar-scroll, home-groups.css);
//   - one tab stop, the current group: ←/→ (↑/↓ in the rail), Home and End
//     move focus through every group and scroll it into view.
// A click switches. A right-click, a long press on touch, or the
// context-menu key (Shift F10) on an icon opens the Groups dialog
// (`GroupEditModal`) with that group selected; the nav header's "…" opens
// it too (`editing`). ⌥⌘1–9 still switch groups (`useWorkspaceIndexShortcuts`).
//
// Presentational: groups and callbacks in, only which dialog is open held
// here when uncontrolled. The logic is `useHomeGroups`. The swipe
// (`useGroupSwipe`) is `swipe` ({progress 0–1, direction next|prev,
// settling while it glides}, null at rest), read from the swipe store
// when the prop is left out. The root carries `data-swiping`,
// `data-swipe-direction` (next, prev or none), `data-swipe-armed` once it
// would commit (past SWIPE_COMMIT_AT, or gliding to land) and
// `--group-swipe-progress` (set from the live swipe in a rAF by
// `group-bar-ink`, not by React). The current icon is `data-swipe-role`
// "from" and the one it heads to "to": the accent blends from one to the
// other with the swipe (home-groups.css), with no hover ink on any icon
// meanwhile, and the bar centres between the two.

import { useEffect, useRef, useState } from 'react'
import { ChevronDownIcon, ChevronLeftIcon, ChevronRightIcon, ChevronUpIcon } from 'lucide-react'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { SWIPE_COMMIT_AT, type SwipeProgress } from '@/lib/group-swipe'
import { useGroupSwipeStore } from '@/hooks/useGroupSwipe'
import { GroupGlyph } from './GroupGlyph'
import {
  GROUP_BAR_GAP_PX,
  GROUP_BAR_GAP_VAR,
  GROUP_BAR_ICON_PX,
  GROUP_BAR_ICON_VAR,
  GROUP_BAR_TARGET_GAP_PX,
  GROUP_BAR_TARGET_PX,
  GROUP_BAR_TARGET_VAR,
  GROUP_RAIL_MAX_PX,
} from './group-bar-look'
import { pageBar, revealInBar, useGroupBarScroll } from './group-bar-scroll'
import { GROUP_SWIPE_PROGRESS_VAR, useGroupSwipeInk } from './group-bar-ink'
import GroupBarTip from './GroupBarTip'
import GroupEditModal from './GroupEditModal'
import './home-groups.css'

export interface GroupBarItem {
  id: string
  name: string
  icon: string | null
  /** Position in the group list (0-based). */
  order: number
  /** "Most recent at the top" is on. */
  recentFirst: boolean
  /** Its pinned rows' addresses, in pin order (at most 9). */
  pinned: readonly string[]
  /** How many agents it holds (the Groups dialog's list shows it). */
  count?: number
}

/** The Groups dialog is open, with this group selected. */
export interface GroupEditing {
  id: string
  /** What opened it: a group's icon (the dialog opens on that group's page
   *  when it is one pane) or the header's "…" (on the list). Focus goes back
   *  there when it closes. Default `icon`. */
  from?: 'icon' | 'header'
}

/** A rename's outcome: `failed` when the name could not be saved (the
 *  Groups dialog keeps the text, edges the field and offers Try again). */
export type GroupRenameResult = 'saved' | 'failed'

export interface HomeGroupBarProps {
  groups: readonly GroupBarItem[]
  currentId: string
  onSelect(id: string): void
  /** Save a name; returns whether it saved (or throws, read as failed). */
  onRename(id: string, name: string): GroupRenameResult
  onSetIcon(id: string, icon: string | null): void
  /** Make a group; returns its id (the dialog selects it), or null. */
  onCreate(): string | null
  onDelete(id: string): void
  /** Undo a delete: the group back at `index`, with its id, rows, pins and
   *  icon. Returns whether it came back. Left out, the dialog offers no Undo. */
  onRestore?(group: GroupBarItem, index: number): boolean
  /** "Most recent at the top" on or off. */
  onSetRecentFirst(id: string, on: boolean): void
  /** The groups in a new order. */
  onReorderGroups(ids: string[]): void
  /** Pins live on the agent rows (the Home nav's row menu and drag); the
   *  bar only shows `pinned`. Here for one contract with the row list. */
  onPin?(groupId: string, rowId: string): void
  onUnpin?(groupId: string, rowId: string): void
  onReorderPins?(groupId: string, rowIds: string[]): void
  /** The swipe; left out, the bar reads the live one (`useGroupSwipeStore`). */
  swipe?: SwipeProgress
  /** `vertical` in the collapsed rail. */
  orientation?: 'horizontal' | 'vertical'
  /** Controlled dialog state (the nav header's "…" opens it too). */
  editing?: GroupEditing | null
  onEditingChange?(next: GroupEditing | null): void
  /** What an empty name saves as (the name field's placeholder). */
  emptyNameFallback?(id: string): string
  /** The switch chord painted in a group's tip (`⌥⌘1`), or null. */
  shortcutLabel?(index: number): string | null
  /** The glyph size (px); GROUP_BAR_ICON_PX unless a side-by-side look
   *  check asks for another. */
  iconPx?: number
}

const LONG_PRESS_MS = 500

const ITEM =
  'no-drag relative flex h-[var(--k2-group-target)] w-[var(--k2-group-target)] flex-shrink-0 items-center justify-center cursor-pointer focus:outline-none transition-colors duration-[var(--motion-duration-fast)] ease-[var(--motion-ease-standard)]'
const ITEM_REST = 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
const ITEM_CURRENT = 'text-[var(--color-accent-text)]'
/** A chevron: bare and muted like the icons, the target's height across the
 *  bar (20 px along it). Shown only while the groups overflow. */
const CHEVRON =
  'no-drag flex flex-shrink-0 cursor-pointer items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors duration-[var(--motion-duration-fast)] ease-[var(--motion-ease-standard)]'
const CHEVRON_PX = 12

/** The group a swipe would land on (no wrap), or null. */
export function swipeTargetId(groups: readonly GroupBarItem[], currentId: string, swipe: SwipeProgress): string | null {
  if (!swipe) return null
  const i = groups.findIndex((g) => g.id === currentId)
  const to = i + (swipe.direction === 'next' ? 1 : -1)
  return i >= 0 && to >= 0 && to < groups.length ? groups[to].id : null
}

/** The swipe would switch if let go now (past the line), or is gliding to land. */
export function swipeArmed(swipe: SwipeProgress | undefined): boolean {
  if (!swipe) return false
  return swipe.settling ? swipe.settling === 'commit' : swipe.progress >= SWIPE_COMMIT_AT
}

/** Where focus goes when the Groups dialog closes: the "…" that opened it,
 *  else the icon of the group it opened on (or the current one). */
function returnFocus(root: HTMLElement | null, editing: GroupEditing, currentId: string): void {
  const icon = (id: string): HTMLElement | null =>
    Array.from(root?.querySelectorAll<HTMLElement>('[data-group-id]') ?? []).find((el) => el.dataset.groupId === id) ?? null
  const target =
    editing.from === 'header'
      ? document.querySelector<HTMLElement>('[data-group-edit-open]')
      : (icon(editing.id) ?? icon(currentId))
  target?.focus({ preventScroll: true })
}

export default function HomeGroupBar({
  groups,
  currentId,
  onSelect,
  onRename,
  onSetIcon,
  onCreate,
  onDelete,
  onRestore,
  onSetRecentFirst,
  onReorderGroups,
  swipe: swipeProp,
  orientation = 'horizontal',
  editing: editingProp,
  onEditingChange,
  shortcutLabel,
  emptyNameFallback,
  iconPx = GROUP_BAR_ICON_PX,
}: HomeGroupBarProps): React.JSX.Element {
  const [editingOwn, setEditingOwn] = useState<GroupEditing | null>(null)
  const controlled = editingProp !== undefined
  const editing = controlled ? editingProp : editingOwn
  const setEditing = (next: GroupEditing | null): void => {
    if (!controlled) setEditingOwn(next)
    onEditingChange?.(next)
  }
  const rootRef = useRef<HTMLDivElement>(null)
  const frameRef = useRef<HTMLDivElement>(null)
  const scrollerRef = useRef<HTMLDivElement>(null)
  const pressTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const [tip, setTip] = useState<{ id: string; el: HTMLElement; keyboard: boolean } | null>(null)
  const vertical = orientation === 'vertical'
  const liveSwipe = useGroupSwipeStore((s) => s.swipe)
  const swipe = swipeProp !== undefined ? swipeProp : liveSwipe
  const armed = swipeArmed(swipe)
  const pendingId = armed ? swipeTargetId(groups, currentId, swipe) : null
  const headingId = swipeTargetId(groups, currentId, swipe)
  // A swipe passed in (a look check) sets the blend itself; the live one paints it.
  useGroupSwipeInk(rootRef, swipeProp === undefined)
  const inkStyle = swipeProp !== undefined ? { [GROUP_SWIPE_PROGRESS_VAR]: swipeProp?.progress ?? 0 } : null

  useGroupBarScroll(frameRef, scrollerRef, vertical, headingId && swipe && swipe.progress > 0 ? [currentId, headingId] : [currentId], `${groups.length}:${iconPx}`)

  const cancelPress = (): void => {
    if (pressTimer.current) clearTimeout(pressTimer.current)
    pressTimer.current = null
  }
  useEffect(() => cancelPress, [])

  const open = (id: string): void => {
    setTip(null)
    setEditing({ id, from: 'icon' })
  }
  const close = (): void => {
    const was = editing
    setEditing(null)
    if (was) requestAnimationFrame(() => returnFocus(rootRef.current, was, currentId))
  }

  // Keyboard focus along the row: the target is focused without the
  // browser's own scroll (which would move the nav too), then the bar
  // scrolls it fully into view.
  const focusItem = (to: HTMLElement | undefined): void => {
    if (!to) return
    to.focus({ preventScroll: true })
    if (frameRef.current && scrollerRef.current) revealInBar(frameRef.current, scrollerRef.current, to, vertical)
  }
  const items = (): HTMLElement[] => Array.from(rootRef.current?.querySelectorAll<HTMLElement>('[data-group-id]') ?? [])
  const focusSibling = (from: HTMLElement, delta: number): void => {
    const all = items()
    const i = all.indexOf(from)
    focusItem(all[Math.min(all.length - 1, Math.max(0, i + delta))])
  }
  const page = (dir: -1 | 1): void => {
    setTip(null)
    if (frameRef.current && scrollerRef.current) pageBar(frameRef.current, scrollerRef.current, vertical, dir)
  }
  const chevron = (dir: -1 | 1): React.JSX.Element => {
    const Icon = vertical ? (dir < 0 ? ChevronUpIcon : ChevronDownIcon) : dir < 0 ? ChevronLeftIcon : ChevronRightIcon
    return (
      <button
        type="button"
        tabIndex={-1}
        aria-hidden="true"
        title={dir < 0 ? 'Earlier groups' : 'More groups'}
        data-group-bar-chevron={dir < 0 ? 'start' : 'end'}
        className={`${CHEVRON} ${vertical ? 'h-5 w-[var(--k2-group-target)]' : 'h-[var(--k2-group-target)] w-5'}`}
        // Pointer only: a press never takes focus from the group that has it.
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => page(dir)}
      >
        <Icon size={CHEVRON_PX} strokeWidth={K2_ICON_STROKE} strokeLinecap="square" strokeLinejoin="miter" aria-hidden="true" />
      </button>
    )
  }

  const tipGroup = tip && !editing && !swipe ? groups.find((g) => g.id === tip.id) : undefined

  return (
    <div
      ref={rootRef}
      role="toolbar"
      aria-label="Groups"
      aria-orientation={orientation}
      data-home-group-bar={orientation}
      data-swiping={swipe ? '' : undefined}
      data-swipe-direction={swipe?.direction ?? 'none'}
      data-swipe-armed={armed ? '' : undefined}
      style={{ ...inkStyle, [GROUP_BAR_ICON_VAR]: `${iconPx}px`, [GROUP_BAR_GAP_VAR]: `${GROUP_BAR_GAP_PX}px`, [GROUP_BAR_TARGET_VAR]: `${GROUP_BAR_TARGET_PX}px` } as React.CSSProperties}
      className={`flex min-w-0 justify-center ${vertical ? 'w-full flex-col items-center' : 'h-[var(--k2-group-target)] w-full flex-row items-center'}`}
    >
      {chevron(-1)}
      <div
        ref={frameRef}
        data-group-bar-frame=""
        data-orientation={orientation}
        className={`relative flex min-w-0 ${vertical ? 'w-full justify-center' : 'max-w-full'}`}
      >
        <div
          ref={scrollerRef}
          className={`relative flex min-h-0 min-w-0 items-center [scrollbar-width:none] [&::-webkit-scrollbar]:hidden ${
            vertical ? 'flex-col overflow-y-auto overflow-x-hidden' : 'h-[var(--k2-group-target)] flex-row overflow-x-auto overflow-y-hidden'
          }`}
          style={vertical ? { maxHeight: GROUP_RAIL_MAX_PX, gap: GROUP_BAR_TARGET_GAP_PX } : { gap: GROUP_BAR_TARGET_GAP_PX }}
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
                data-pending={g.id === pendingId ? '' : undefined}
                data-swipe-role={headingId ? (current ? 'from' : g.id === headingId ? 'to' : undefined) : undefined}
                data-recent-first={g.recentFirst ? '' : undefined}
                data-k2-focus=""
                // One tab stop, the current group; the arrows reach the rest.
                tabIndex={current ? 0 : -1}
                aria-label={g.name}
                aria-description={g.recentFirst ? 'Most recent at the top' : undefined}
                aria-keyshortcuts={chord ? chordToAria(chord) : undefined}
                aria-current={current ? 'true' : undefined}
                aria-haspopup="dialog"
                className={`${ITEM} ${current ? ITEM_CURRENT : ITEM_REST}`}
                onClick={() => onSelect(g.id)}
                onContextMenu={(e) => {
                  e.preventDefault()
                  open(g.id)
                }}
                onMouseEnter={(e) => setTip({ id: g.id, el: e.currentTarget, keyboard: false })}
                onMouseLeave={() => setTip((t) => (t && !t.keyboard ? null : t))}
                onFocus={(e) => {
                  if (e.currentTarget.matches(':focus-visible')) setTip({ id: g.id, el: e.currentTarget, keyboard: true })
                }}
                onBlur={() => setTip((t) => (t?.keyboard ? null : t))}
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
                  } else if (e.key === 'Home' || e.key === 'End') {
                    e.preventDefault()
                    const all = items()
                    focusItem(e.key === 'Home' ? all[0] : all[all.length - 1])
                  } else if (e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey)) {
                    e.preventDefault()
                    open(g.id)
                  }
                }}
              >
                <GroupGlyph icon={g.icon} index={i} size={iconPx} bare />
              </button>
            )
          })}
        </div>
        <span aria-hidden="true" data-group-bar-fade="start" />
        <span aria-hidden="true" data-group-bar-fade="end" />
      </div>
      {chevron(1)}

      {tip && tipGroup && (
        <GroupBarTip
          anchor={tip.el}
          name={tipGroup.name}
          chord={shortcutLabel?.(groups.indexOf(tipGroup)) ?? null}
          keyboard={tip.keyboard}
          vertical={vertical}
        />
      )}

      {editing && (
        <GroupEditModal
          groups={groups}
          currentId={currentId}
          initialId={editing.id}
          startOn={editing.from === 'header' ? 'list' : 'group'}
          startTab={editing.from === 'header' ? 'agents' : 'icon'}
          onRename={onRename}
          onSetIcon={onSetIcon}
          onCreate={onCreate}
          onDelete={onDelete}
          onRestore={onRestore}
          onSetRecentFirst={onSetRecentFirst}
          onReorderGroups={onReorderGroups}
          emptyNameFallback={emptyNameFallback}
          onClose={close}
        />
      )}
    </div>
  )
}

/** `⌥⌘1` as aria-keyshortcuts reads it (`Alt+Meta+1`). */
function chordToAria(chord: string): string {
  const keys: string[] = []
  let rest = chord
  for (const [sym, name] of [['⌃', 'Control'], ['⌥', 'Alt'], ['⇧', 'Shift'], ['⌘', 'Meta']] as const) {
    if (rest.includes(sym)) {
      keys.push(name)
      rest = rest.replace(sym, '')
    }
  }
  return [...keys, rest.replace(/\+/g, '').trim()].filter(Boolean).join('+')
}
